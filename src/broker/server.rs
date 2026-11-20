//! 在同一登入工作階段處理查詢並持有候選快照。
use std::borrow::Cow;
use std::collections::VecDeque;
use std::os::windows::io::OwnedHandle;
use std::ptr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use super::modes::{ModeAction, ModeStore};
use super::permissions;
use super::pipe::{
    client_process, connect, create_pipe, create_stop_event, make_security, open_anchor, pipe_name,
    project_id, read_all, request_stop, stopped, write_all, Raw,
};
use super::protocol::{
    add_text, read_u32, Operation, EXTENDED_MODES, LOOKUP_MODE_SHIFT, MAGIC, MAX_BODY, MAX_COUNT,
    MAX_PREFIX, RAW_STROKE_KEYS,
};
use crate::diagnostics::{self, Level, WarningLimit};
use crate::dictionary::{Dictionary, DictionaryMatch, DictionaryStore, InputMode};
use crate::engine::preview;
use crate::project_files;
use crate::simplify::{self, Converter};

const MAX_CLIENTS: usize = 8;
const MAX_SNAPSHOTS: usize = 512;
const MAX_SNAPSHOT_INDICES: usize = 2_000_000;
const SNAPSHOT_TTL: Duration = Duration::from_secs(3600);

#[link(name = "bcrypt")]
extern "system" {
    fn BCryptGenRandom(algorithm: Raw, buffer: *mut u8, length: u32, flags: u32) -> i32;
}

#[derive(Clone)]
struct Client {
    process: u32,
    sid: Option<String>,
}

struct Snapshot {
    token: u64,
    client: Client,
    created: Instant,
    dictionary: Arc<Dictionary>,
    matches: Vec<DictionaryMatch>,
    converter: Option<Arc<Converter>>,
}

struct Selection {
    dictionary: Arc<Dictionary>,
    entry: DictionaryMatch,
    converter: Option<Arc<Converter>>,
}

struct BrokerState {
    dictionary: Mutex<DictionaryStore>,
    snapshots: Mutex<VecDeque<Snapshot>>,
    permissions: permissions::Permissions,
    modes: Mutex<ModeStore>,
}

impl Snapshot {
    fn active(&self) -> bool {
        self.created.elapsed() < SNAPSHOT_TTL
    }

    fn available_to(&self, client: &Client) -> bool {
        self.client.process == client.process && self.client.sid == client.sid && self.active()
    }
}

impl BrokerState {
    fn dictionary(&self) -> Result<Arc<Dictionary>, String> {
        self.dictionary
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .refresh()
    }

    fn lookup(
        &self,
        client: &Client,
        code: &str,
        flags: u8,
        mode: InputMode,
        with_commands: bool,
        body: &mut Vec<u8>,
    ) -> Result<usize, String> {
        let dictionaries = self
            .dictionary
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .dictionaries(mode)?;
        let dictionary = dictionaries.table(mode).ok_or("無法讀取筆劃字庫")?.clone();
        let converter = (flags & 2 != 0).then(simplify::current);
        let mut matches = dictionary.lookup(
            mode,
            code,
            flags & 1 != 0,
            converter.as_deref(),
            &dictionaries.ranking,
        );
        if !with_commands {
            // RI01 的舊操作只回傳詞條；新版客戶端在宿主內產生命令結果。
            matches.retain(|item| dictionary.command(*item).is_none());
        }
        let count = matches.len();
        if count > MAX_COUNT {
            return Err("候選數量過多".into());
        }
        body.extend_from_slice(&0u64.to_le_bytes());
        for &item in &matches {
            let value = dictionary.text(item, converter.as_deref());
            if dictionary.command(item).is_some() {
                add_text(body, &value)?;
            } else {
                add_text(body, &preview(&value, mode))?;
            }
        }
        if count > 0 {
            // 預覽已序列化，持鎖期間只管理快照，其他客戶可同時查字。
            let mut snapshots = self
                .snapshots
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            snapshots.retain(Snapshot::active);
            let token = loop {
                let token = new_snapshot_token()?;
                if !snapshots.iter().any(|entry| entry.token == token) {
                    break token;
                }
            };
            let mut indices = snapshots
                .iter()
                .map(|item| item.matches.len())
                .sum::<usize>();
            while snapshots.len() >= MAX_SNAPSHOTS || indices + count > MAX_SNAPSHOT_INDICES {
                let removed = snapshots.pop_front().expect("快照上限超出");
                indices -= removed.matches.len();
            }
            snapshots.push_back(Snapshot {
                token,
                client: client.clone(),
                created: Instant::now(),
                dictionary,
                matches,
                converter,
            });
            body[..8].copy_from_slice(&token.to_le_bytes());
        }
        Ok(count)
    }

    fn selected(&self, client: &Client, token: u64, index: usize) -> Result<Selection, String> {
        let snapshots = self
            .snapshots
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let snapshot = snapshots
            .iter()
            .find(|entry| entry.token == token && entry.available_to(client));
        if snapshot.is_none() {
            diagnostics::event(Level::Detail, "broker", "snapshot_unavailable", &[]);
        }
        let snapshot = snapshot.ok_or("候選快照已過期")?;
        let entry = *snapshot.matches.get(index).ok_or("候選編號無效")?;
        Ok(Selection {
            dictionary: snapshot.dictionary.clone(),
            entry,
            converter: snapshot.converter.clone(),
        })
    }
}

fn new_snapshot_token() -> Result<u64, String> {
    let mut token = [0u8; 8];
    loop {
        if unsafe { BCryptGenRandom(ptr::null_mut(), token.as_mut_ptr(), token.len() as u32, 2) }
            < 0
        {
            return Err("無法產生候選快照識別碼".into());
        }
        let id = u64::from_le_bytes(token);
        if id != 0 {
            return Ok(id);
        }
    }
}

fn handle_client(pipe: OwnedHandle, state: Arc<BrokerState>) -> Result<(), String> {
    use Operation::*;

    let process = client_process(&pipe)?;
    let mut deadline = Instant::now() + Duration::from_secs(2);
    let mut header = [0u8; 8];
    read_all(&pipe, &mut header, deadline)?;
    if &header[..4] != MAGIC {
        return Err("查字要求無效".into());
    }
    let flags = header[4];
    let operation = Operation::try_from(header[5])?;
    let mut length = u16::from_le_bytes([header[6], header[7]]) as usize;
    if operation == ConvertClipboard {
        deadline = Instant::now() + Duration::from_secs(5);
    }
    if operation == ConvertClipboard && length == 0 {
        let mut extended = [0u8; 4];
        read_all(&pipe, &mut extended, deadline)?;
        length = read_u32(&extended) as usize;
    }
    if length
        > if operation == ConvertClipboard {
            MAX_BODY
        } else {
            MAX_PREFIX
        }
    {
        return Err("查字要求無效".into());
    }
    let mut payload = vec![0; length];
    read_all(&pipe, &mut payload, deadline)?;
    let sid = permissions::client_sid(&pipe)?;
    let client = Client { process, sid };
    diagnostics::event(
        Level::Detail,
        "broker",
        "request",
        &[
            ("client_pid", i64::from(process)),
            ("sandboxed", i64::from(client.sid.is_some())),
            ("operation", operation as i64),
        ],
    );
    let mut body = Vec::new();
    let count = match operation {
        LegacyLookup | Lookup => {
            let raw_stroke_keys = flags & RAW_STROKE_KEYS != 0;
            let mode = InputMode::from_id((flags & !RAW_STROKE_KEYS) >> LOOKUP_MODE_SHIFT)
                .filter(|mode| {
                    if raw_stroke_keys {
                        operation == Lookup
                            && *mode == InputMode::Stroke
                            && mode.accepts_lookup(&payload)
                    } else {
                        (operation == Lookup || *mode == InputMode::Cangjie)
                            && mode.accepts_code(&payload)
                    }
                })
                .ok_or("查字編碼無效")?;
            let code = std::str::from_utf8(&payload).map_err(|_| "查字編碼無效")?;
            let code = if mode == InputMode::Stroke && !raw_stroke_keys {
                // 舊 DLL 送出碼表符號。用等價的小鍵盤編碼查原筆劃候選。
                Cow::Owned(
                    code.bytes()
                        .map(|byte| match byte {
                            b'h' => '7',
                            b's' => '8',
                            b'p' => '9',
                            b'n' => '4',
                            b'z' => '5',
                            b'*' => '6',
                            _ => unreachable!("筆劃碼已驗證"),
                        })
                        .collect::<String>(),
                )
            } else {
                Cow::Borrowed(code)
            };
            state.lookup(&client, &code, flags, mode, operation == Lookup, &mut body)?
        }
        Resolve | ReverseMatch
            if length == 12 && (flags == 0 || (operation == Resolve && flags == 1)) =>
        {
            let token = u64::from_le_bytes(payload[..8].try_into().expect("八位元組"));
            let index = read_u32(&payload[8..]) as usize;
            let Selection {
                dictionary,
                entry,
                converter,
            } = state.selected(&client, token, index)?;
            let text = if operation == ReverseMatch {
                Cow::Owned(dictionary.reverse(&dictionary.original(entry)))
            } else if flags == 1 {
                dictionary.original(entry)
            } else {
                dictionary.text(entry, converter.as_deref())
            };
            add_text(&mut body, &text)?;
            1
        }
        PermissionStatus if flags == 0 && length == 0 => {
            // 舊版探測仍可用。詞典查詢不再依賴此值。
            body.push(u8::from(
                client
                    .sid
                    .as_deref()
                    .is_some_and(|sid| state.permissions.enabled(sid)),
            ));
            0
        }
        Reverse if flags == 0 && length > 0 => {
            let text = std::str::from_utf8(&payload).map_err(|_| "反查文字無效")?;
            let dictionary = state.dictionary()?;
            add_text(&mut body, &dictionary.reverse(text))?;
            1
        }
        ConvertClipboard
            if flags == 0
                && length > 0
                && client
                    .sid
                    .as_deref()
                    .is_some_and(|sid| state.permissions.enabled(sid)) =>
        {
            let text = std::str::from_utf8(&payload).map_err(|_| "轉換文字編碼無效")?;
            let converted =
                crate::commands::convert_clipboard_text(text).ok_or("無法讀取私人轉換規則")?;
            add_text(&mut body, &converted)?;
            1
        }
        Modes if length == 0 => {
            let extended = flags & EXTENDED_MODES != 0;
            let value = state
                .modes
                .lock()
                .unwrap_or_else(|err| err.into_inner())
                .apply(ModeAction::try_from(flags & !EXTENDED_MODES)?)?;
            // 舊版 DLL 仍在宿主中時，只向它回傳原有的五個位元。
            body.push((value.bits() as u8) & if extended { 0x7f } else { 0x1f });
            0
        }
        Jyutping | Homophones if length > 0 => {
            let input = std::str::from_utf8(&payload).map_err(|_| "字音查詢編碼無效")?;
            let exact_lookup = operation == Jyutping;
            if exact_lookup && (flags > 1 || !input.bytes().all(|ch| ch.is_ascii_lowercase())) {
                return Err("拼音查詢無效".into());
            }
            if !exact_lookup && input.chars().count() > 20 {
                0
            } else {
                let fuzzy = state
                    .dictionary
                    .lock()
                    .unwrap_or_else(|err| err.into_inner())
                    .fuzzy();
                let entries = if exact_lookup {
                    crate::homophone::reverse(&fuzzy, input, flags != 0)
                } else {
                    crate::homophone::lookup(&fuzzy, input, flags as usize)
                };
                let count = entries.len() * 2;
                for (text, comment) in entries {
                    add_text(&mut body, &text)?;
                    add_text(&mut body, &comment)?;
                }
                count
            }
        }
        _ => return Err("查字要求無效".into()),
    };
    let mut response = [0u8; 16];
    response[..4].copy_from_slice(MAGIC);
    response[8..12].copy_from_slice(&(count as u32).to_le_bytes());
    response[12..].copy_from_slice(&(body.len() as u32).to_le_bytes());
    write_all(&pipe, &response, deadline)?;
    write_all(&pipe, &body, deadline)?;
    Ok(())
}

#[cfg(feature = "integration-fixtures")]
pub(crate) fn set_test_permission(sid: &str, enabled: bool) -> Result<(), String> {
    if !sid.starts_with("S-1-15-2-") || !sid.bytes().all(|ch| b"S-0123456789".contains(&ch)) {
        return Err("無效測試 SID".into());
    }
    permissions::Permissions::new(project_id(&project_files::executable_root()?))
        .write(sid, enabled)
}

pub(crate) fn serve(replace: bool) -> Result<(), String> {
    let root = project_files::executable_root()?;
    let name = pipe_name(&root)?;
    let security = make_security()?;
    if replace {
        request_stop(&root)?;
    }
    // 保留第一個實例直至程序結束，避免其他程序在請求之間佔用管道名稱。
    // 它必須先接上本程序的客戶端，否則外來查詢可能連到無人處理的保留實例。
    let deadline = replace.then(|| Instant::now() + Duration::from_secs(5));
    let anchor = create_pipe(&name, &security, true, deadline)?;
    let _anchor_client = open_anchor(&name)?;
    connect(&anchor, None)?;
    let stop = create_stop_event(&root)?;
    let id = project_id(&root);
    let state = Arc::new(BrokerState {
        dictionary: Mutex::new(DictionaryStore::for_root(&root)?),
        snapshots: Mutex::new(VecDeque::new()),
        permissions: permissions::Permissions::new(id),
        modes: Mutex::new(ModeStore::new(id, &root)?),
    });
    diagnostics::event(Level::Info, "broker", "ready", &[]);
    static PIPE_FAILURE: WarningLimit = WarningLimit::new();
    static CLIENT_LIMIT: WarningLimit = WarningLimit::new();
    static REQUEST_FAILURE: WarningLimit = WarningLimit::new();
    let clients = Arc::new(AtomicUsize::new(0));
    while !stopped(&stop)? {
        let pipe = match create_pipe(&name, &security, false, None) {
            Ok(pipe) => pipe,
            Err(_) => {
                if PIPE_FAILURE.ready() {
                    diagnostics::event(Level::Error, "broker", "create_pipe_failed", &[]);
                }
                std::thread::sleep(Duration::from_millis(20));
                continue;
            }
        };
        match connect(&pipe, Some(&stop)) {
            Ok(true) => (),
            Ok(false) => break,
            Err(_) => {
                if PIPE_FAILURE.ready() {
                    diagnostics::event(Level::Warning, "broker", "connect_failed", &[]);
                }
                continue;
            }
        }
        if clients.load(Ordering::Relaxed) >= MAX_CLIENTS {
            if CLIENT_LIMIT.ready() {
                diagnostics::event(
                    Level::Warning,
                    "broker",
                    "client_limit",
                    &[("limit", MAX_CLIENTS as i64)],
                );
            }
            continue;
        }
        clients.fetch_add(1, Ordering::Relaxed);
        let state = state.clone();
        let clients = clients.clone();
        std::thread::spawn(move || {
            if handle_client(pipe, state).is_err() && REQUEST_FAILURE.ready() {
                diagnostics::event(Level::Warning, "broker", "request_failed", &[]);
            }
            clients.fetch_sub(1, Ordering::Relaxed);
        });
    }
    diagnostics::event(Level::Info, "broker", "stop_requested", &[]);
    Ok(())
}
