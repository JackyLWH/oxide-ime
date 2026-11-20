//! 沙盒宿主使用的字庫查詢接口。
use std::sync::Arc;
use std::time::{Duration, Instant};

use super::pipe::{client_pipe_name, open_client, pipe_name, read_all, server_process, write_all};
use super::protocol::{
    decode_response, decode_single, read_u32, Operation, MAGIC, MAX_BODY, MAX_PREFIX, TIMEOUT,
};
use super::{ModeAction, Modes};
use crate::diagnostics::{self, Level, WarningLimit};
use crate::project_files;

fn request(
    name: &[u16],
    operation: Operation,
    flags: u8,
    payload: &[u8],
) -> Result<(usize, Vec<u8>), String> {
    request_from(name, operation, flags, payload, None)
}

fn request_from(
    name: &[u16],
    operation: Operation,
    flags: u8,
    payload: &[u8],
    expected_pid: Option<u32>,
) -> Result<(usize, Vec<u8>), String> {
    static FAILED: WarningLimit = WarningLimit::new();
    let started = Instant::now();
    let mut phase = 0;
    let result = exchange(name, operation, flags, payload, expected_pid, &mut phase);
    if result.is_ok() || FAILED.ready() {
        let (count, bytes) = result
            .as_ref()
            .map_or((0, 0), |(count, body)| (*count, body.len()));
        diagnostics::event(
            if result.is_ok() {
                Level::Detail
            } else {
                Level::Warning
            },
            "broker_client",
            "request",
            &[
                ("operation", operation as i64),
                ("phase", phase),
                ("ok", i64::from(result.is_ok())),
                ("response_count", count as i64),
                ("response_bytes", bytes as i64),
                ("elapsed_ms", diagnostics::millis(started.elapsed())),
            ],
        );
    }
    result
}

fn exchange(
    name: &[u16],
    operation: Operation,
    flags: u8,
    payload: &[u8],
    expected_pid: Option<u32>,
    phase: &mut i64,
) -> Result<(usize, Vec<u8>), String> {
    let clipboard = operation == Operation::ConvertClipboard;
    if payload.len() > MAX_BODY || (payload.len() > u16::MAX as usize && !clipboard) {
        return Err("查字要求過長".into());
    }
    let deadline = Instant::now()
        + if operation == Operation::Modes {
            if flags == 0 {
                Duration::from_millis(30)
            } else {
                Duration::from_millis(150)
            }
        } else if clipboard {
            Duration::from_secs(5)
        } else {
            TIMEOUT
        };
    *phase = 1;
    let pipe = open_client(name, deadline)?;
    let responding_pid = expected_pid.map(|_| server_process(&pipe)).transpose()?;
    let extended = clipboard && payload.len() > u16::MAX as usize;
    let mut request = Vec::with_capacity(12 + payload.len());
    request.extend_from_slice(MAGIC);
    request.push(flags);
    request.push(operation as u8);
    request.extend_from_slice(&(if extended { 0 } else { payload.len() as u16 }).to_le_bytes());
    if extended {
        request.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    }
    request.extend_from_slice(payload);
    *phase = 2;
    write_all(&pipe, &request, deadline)?;
    let mut header = [0u8; 16];
    *phase = 3;
    read_all(&pipe, &mut header, deadline)?;
    if &header[..4] != MAGIC || read_u32(&header[4..8]) != 0 {
        return Err("字庫未能處理查詢".into());
    }
    let count = read_u32(&header[8..12]) as usize;
    let length = read_u32(&header[12..16]) as usize;
    if length > MAX_BODY {
        return Err("候選資料過長".into());
    }
    let mut body = vec![0; length];
    *phase = 4;
    read_all(&pipe, &mut body, deadline)?;
    if responding_pid != expected_pid {
        return Err("命名管道尚未由新字庫服務接手".into());
    }
    Ok((count, body))
}

pub(crate) struct RemoteCandidates {
    pub snapshot: u64,
    pub previews: Vec<Arc<str>>,
}

pub(crate) fn lookup(
    prefix: &str,
    limit: bool,
    simplified: bool,
) -> Result<RemoteCandidates, String> {
    if prefix.len() > MAX_PREFIX {
        return Ok(RemoteCandidates {
            snapshot: 0,
            previews: Vec::new(),
        });
    }
    if prefix.is_empty() || !prefix.bytes().all(|byte| byte.is_ascii_lowercase()) {
        return Err("查字編碼無效".into());
    }
    let (count, body) = request(
        client_pipe_name()?,
        Operation::Lookup,
        u8::from(limit) | (u8::from(simplified) << 1),
        prefix.as_bytes(),
    )?;
    let snapshot = u64::from_le_bytes(
        body.get(..8)
            .ok_or("候選資料不完整")?
            .try_into()
            .expect("八位元組"),
    );
    if (snapshot == 0) != (count == 0) {
        return Err("候選快照無效".into());
    }
    let previews = decode_response(&body[8..], count)?
        .into_iter()
        .map(Arc::from)
        .collect();
    Ok(RemoteCandidates { snapshot, previews })
}

pub(crate) fn resolve(snapshot: u64, index: u32) -> Result<String, String> {
    snapshot_text(Operation::Resolve, snapshot, index, 0)
}

pub(crate) fn resolve_original(snapshot: u64, index: u32) -> Result<String, String> {
    snapshot_text(Operation::Resolve, snapshot, index, 1)
}

pub(crate) fn reverse(text: &str) -> Result<String, String> {
    if text.is_empty() || text.len() > MAX_PREFIX {
        return Ok(String::new());
    }
    let (count, body) = request(client_pipe_name()?, Operation::Reverse, 0, text.as_bytes())?;
    decode_single(&body, count, "反查字碼資料無效")
}

pub(crate) fn reverse_match(snapshot: u64, index: u32) -> Result<String, String> {
    snapshot_text(Operation::ReverseMatch, snapshot, index, 0)
}

pub(crate) fn modes(action: ModeAction) -> Result<Modes, String> {
    mode_request(client_pipe_name()?, action, None)
}

pub(crate) fn clipboard_enabled() -> bool {
    client_pipe_name()
        .and_then(|name| request(name, Operation::PermissionStatus, 0, &[]))
        .is_ok_and(|(count, body)| count == 0 && body == [1])
}

fn mode_request(
    name: &[u16],
    action: ModeAction,
    expected_pid: Option<u32>,
) -> Result<Modes, String> {
    let (count, body) = request_from(name, Operation::Modes, action as u8, &[], expected_pid)?;
    match body.as_slice() {
        [bits @ 0..=31] if count == 0 => Ok(Modes::from_bits(u32::from(*bits))),
        _ => Err("輸入模式狀態無效".into()),
    }
}

fn phonetic_request(
    operation: Operation,
    flags: u8,
    input: &str,
) -> Result<Vec<(String, String)>, String> {
    if input.is_empty() || input.len() > MAX_PREFIX {
        return Ok(Vec::new());
    }
    let (count, body) = request(client_pipe_name()?, operation, flags, input.as_bytes())?;
    if count % 2 != 0 {
        return Err("字音候選資料不完整".into());
    }
    Ok(decode_response(&body, count)?
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| (pair[0].to_string(), pair[1].to_string()))
        .collect())
}

pub(crate) fn jyutping(code: &str, exact: bool) -> Result<Vec<(String, String)>, String> {
    phonetic_request(Operation::Jyutping, u8::from(exact), code)
}

pub(crate) fn homophones(text: &str, tone: usize) -> Result<Vec<(String, String)>, String> {
    phonetic_request(
        Operation::Homophones,
        tone.min(u8::MAX as usize) as u8,
        text,
    )
}

pub(crate) fn convert_clipboard(text: &str) -> Result<String, String> {
    if text.is_empty() {
        return Err("剪貼簿文字為空".into());
    }
    let (count, body) = request(
        client_pipe_name()?,
        Operation::ConvertClipboard,
        0,
        text.as_bytes(),
    )?;
    decode_single(&body, count, "轉換結果無效")
}

fn snapshot_text(
    operation: Operation,
    snapshot: u64,
    index: u32,
    flags: u8,
) -> Result<String, String> {
    let mut payload = [0u8; 12];
    payload[..8].copy_from_slice(&snapshot.to_le_bytes());
    payload[8..].copy_from_slice(&index.to_le_bytes());
    let (count, body) = request(client_pipe_name()?, operation, flags, &payload)?;
    decode_single(&body, count, "選取的候選無效")
}

pub(crate) fn check(expected_pid: Option<u32>) -> Result<(), String> {
    let name = pipe_name(&project_files::executable_root()?)?;
    mode_request(&name, ModeAction::Read, expected_pid).map(|_| ())
}
