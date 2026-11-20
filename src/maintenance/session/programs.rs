//! 以執行檔及程序建立時間識別新開啟的指定程式。
use std::collections::{HashMap, HashSet};
use std::ffi::OsString;
use std::os::windows::ffi::OsStringExt;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, UNIX_EPOCH};

use serde::Deserialize;

use crate::tsf::ffi::Raw;
use crate::win32::GetForegroundWindow;

#[link(name = "kernel32")]
extern "system" {
    fn OpenProcess(access: u32, inherit: i32, pid: u32) -> Raw;
    fn GetProcessTimes(
        process: Raw,
        created: *mut u64,
        exited: *mut u64,
        kernel: *mut u64,
        user: *mut u64,
    ) -> i32;
    fn QueryFullProcessImageNameW(
        process: Raw,
        flags: u32,
        path: *mut u16,
        length: *mut u32,
    ) -> i32;
    fn GetSystemTimeAsFileTime(time: *mut u64);
}

#[link(name = "user32")]
extern "system" {
    fn IsWindowVisible(window: Raw) -> i32;
    fn IsIconic(window: Raw) -> i32;
    fn GetWindowThreadProcessId(window: Raw, pid: *mut u32) -> u32;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) struct ProcessKey {
    pid: u32,
    created: u64,
}

pub(super) struct Process {
    pub(super) key: ProcessKey,
    image_key: Option<String>,
}

impl Process {
    fn foreground() -> Option<Self> {
        let pid = foreground_pid()?;
        let (key, process) = process_key(pid)?;
        let mut path = vec![0u16; 32768];
        let mut length = path.len() as u32;
        if unsafe {
            QueryFullProcessImageNameW(process.as_raw_handle(), 0, path.as_mut_ptr(), &mut length)
        } == 0
        {
            return None;
        }
        let image = OsString::from_wide(&path[..length as usize]);
        Some(Self {
            key,
            image_key: path_key(Path::new(&image)),
        })
    }

    pub(super) fn is_current(&self) -> bool {
        foreground_pid() == Some(self.key.pid)
            && process_key(self.key.pid).is_some_and(|(key, _)| key == self.key)
    }
}

fn foreground_pid() -> Option<u32> {
    unsafe {
        let window = GetForegroundWindow();
        if window.is_null() || IsWindowVisible(window) == 0 || IsIconic(window) != 0 {
            return None;
        }
        let mut pid = 0;
        GetWindowThreadProcessId(window, &mut pid);
        (pid != 0).then_some(pid)
    }
}

fn process_key(pid: u32) -> Option<(ProcessKey, OwnedHandle)> {
    let raw = unsafe { OpenProcess(0x1000, 0, pid) }; // PROCESS_QUERY_LIMITED_INFORMATION
    if raw.is_null() {
        return None;
    }
    let process = unsafe { OwnedHandle::from_raw_handle(raw) };
    let (mut created, mut exited, mut kernel, mut user) = (0, 0, 0, 0);
    (unsafe {
        GetProcessTimes(
            process.as_raw_handle(),
            &mut created,
            &mut exited,
            &mut kernel,
            &mut user,
        )
    } != 0)
        .then_some((ProcessKey { pid, created }, process))
}

fn filetime_now() -> u64 {
    let mut now = 0;
    unsafe { GetSystemTimeAsFileTime(&mut now) };
    now
}

const LOL_IMAGE: &str = "league of legends.exe";

fn path_key(path: &Path) -> Option<String> {
    if path.to_str()?.eq_ignore_ascii_case(LOL_IMAGE) {
        return Some(LOL_IMAGE.to_owned());
    }
    if !path.is_absolute()
        || !path
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| extension.eq_ignore_ascii_case("exe"))
    {
        return None;
    }
    let path = path.to_str()?.replace('/', "\\");
    let path = path.strip_prefix(r"\\?\").unwrap_or(&path);
    Some(path.to_lowercase())
}

#[derive(Deserialize)]
struct Rule {
    path: PathBuf,
    delay_ms: u64,
}

struct Monitored {
    since: u64,
    delay: Duration,
}

#[derive(Default)]
struct Focus {
    pending: Option<(ProcessKey, Instant)>,
}

impl Focus {
    fn ready(&mut self, key: ProcessKey, now: Instant, delay: Duration) -> bool {
        let since = match self.pending {
            Some((focused, since)) if focused == key => since,
            _ => {
                self.pending = Some((key, now));
                now
            }
        };
        now.duration_since(since) >= delay
    }
}

pub(super) struct Watch {
    started: u64,
    rules: HashMap<String, Monitored>,
    handled: HashSet<ProcessKey>,
    focus: Focus,
    process: Option<Process>,
}

impl Watch {
    pub(super) fn new() -> Self {
        Self {
            started: filetime_now(),
            rules: HashMap::new(),
            handled: HashSet::new(),
            focus: Focus::default(),
            process: None,
        }
    }

    pub(super) fn reload(&mut self, directory: &Path) {
        let path = directory.join("restart-programs.json");
        let loaded = match std::fs::read(&path) {
            Ok(bytes) if bytes.len() <= 256 * 1024 => serde_json::from_slice::<Vec<Rule>>(
                bytes.strip_prefix(b"\xef\xbb\xbf").unwrap_or(&bytes),
            )
            .ok(),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                serde_json::from_str(crate::maintenance::DEFAULT_RESTART_PROGRAMS).ok()
            }
            _ => None,
        };
        let Some(loaded) = loaded else { return };
        let modified = std::fs::metadata(path)
            .ok()
            .and_then(|metadata| metadata.modified().ok())
            .and_then(|modified| modified.duration_since(UNIX_EPOCH).ok())
            .and_then(|elapsed| u64::try_from(elapsed.as_nanos() / 100).ok())
            .and_then(|unix| unix.checked_add(116_444_736_000_000_000))
            .unwrap_or(self.started);
        let since = modified.max(self.started);
        let mut rules = HashMap::new();
        for rule in loaded.into_iter().take(128) {
            if let Some(path) = path_key(&rule.path).filter(|_| rule.delay_ms <= 600_000) {
                let since = self.rules.get(&path).map_or(since, |old| old.since);
                rules.insert(
                    path,
                    Monitored {
                        since,
                        delay: Duration::from_millis(rule.delay_ms),
                    },
                );
            }
        }
        self.rules = rules;
        self.handled
            .retain(|key| process_key(key.pid).is_some_and(|(live, _)| live == *key));
    }

    pub(super) fn is_empty(&self) -> bool {
        self.rules.is_empty()
    }

    pub(super) fn ready(&mut self, now: Instant) -> Option<&Process> {
        if self.is_empty() {
            self.focus.pending = None;
            return None;
        }
        if !self.process.as_ref().is_some_and(Process::is_current) {
            self.process = Process::foreground();
        }
        let eligible = self.process.as_ref().and_then(|process| {
            let image = process.image_key.as_deref()?;
            let rule = self.rules.get(image).or_else(|| {
                let name = image.rsplit('\\').next()?;
                (name == LOL_IMAGE)
                    .then(|| self.rules.get(LOL_IMAGE))
                    .flatten()
            })?;
            (process.key.created >= rule.since && !self.handled.contains(&process.key))
                .then_some((process, rule.delay))
        });
        let Some((process, delay)) = eligible else {
            self.focus.pending = None;
            return None;
        };
        self.focus.ready(process.key, now, delay).then_some(process)
    }

    pub(super) fn handled(&mut self, key: ProcessKey) {
        self.handled.insert(key);
        self.focus.pending = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn foreground_wait_restarts_when_focus_changes_and_supports_zero() {
        let mut focus = Focus::default();
        let program = ProcessKey { pid: 3, created: 5 };
        let reused_pid = ProcessKey { pid: 3, created: 6 };
        let since = Instant::now();
        let delay = Duration::from_millis(1500);
        assert!(!focus.ready(program, since, delay));
        assert!(!focus.ready(program, since + Duration::from_secs(1), delay));
        assert!(focus.ready(program, since + delay, delay));
        assert!(!focus.ready(reused_pid, since + delay, delay));
        assert!(focus.ready(reused_pid, since + delay, Duration::ZERO));
        focus.pending = None;
        assert!(!focus.ready(program, since + delay * 2, delay));
        assert_ne!(
            path_key(Path::new(r"C:\one\Game.exe")),
            path_key(Path::new(r"C:\two\Game.exe"))
        );
    }
}
