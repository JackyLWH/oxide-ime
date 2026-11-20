//! 動態命令；剪貼簿只由目前宿主以 Windows 權限讀取，永不透過字庫服務讀取。
use crate::conversion::{replace_prefixes, PhraseTable};
#[cfg(not(feature = "integration-fixtures"))]
use std::ffi::c_void;
#[cfg(not(feature = "integration-fixtures"))]
use std::ptr;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Instant;
use std::{borrow::Cow, collections::HashMap};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Command {
    ClipboardRaw,
    ClipboardConverted,
    Time,
    Date,
}

impl Command {
    pub(crate) const MARKERS: [(&'static str, Self); 4] = [
        ("__rime_clipboard_raw__", Self::ClipboardRaw),
        ("__rime_clipboard_s2t__", Self::ClipboardConverted),
        ("__rime_time__", Self::Time),
        ("__rime_date__", Self::Date),
    ];

    pub(crate) fn from_marker(text: &str) -> Option<Self> {
        Self::MARKERS
            .into_iter()
            .find_map(|(marker, command)| (text == marker).then_some(command))
    }

    pub(crate) fn code(self) -> &'static str {
        match self {
            Self::ClipboardRaw | Self::ClipboardConverted => "zv",
            Self::Time => "ztime",
            Self::Date => "zdate",
        }
    }
}

pub(crate) struct ClipboardSnapshot {
    pub raw: Arc<str>,
    pub converted: Option<Arc<str>>,
    simplified: OnceLock<Arc<str>>,
}

impl ClipboardSnapshot {
    pub(crate) fn new(raw: Arc<str>, converted: Option<Arc<str>>) -> Self {
        Self {
            raw,
            converted,
            simplified: OnceLock::new(),
        }
    }

    pub(crate) fn simplified(&self, converter: &crate::simplify::Converter) -> Arc<str> {
        self.simplified
            .get_or_init(|| match converter.convert(&self.raw) {
                Cow::Borrowed(_) => self.raw.clone(),
                Cow::Owned(converted) => converted.into(),
            })
            .clone()
    }
}

pub(crate) trait CommandProvider: Send + Sync {
    fn clipboard(&self) -> Option<ClipboardSnapshot>;
    fn time(&self) -> String;
    fn date(&self) -> String;
}

pub(crate) struct WindowsCommands {
    pub sandboxed: bool,
}

#[cfg(not(feature = "integration-fixtures"))]
#[link(name = "kernel32")]
extern "system" {
    fn GlobalSize(memory: *mut c_void) -> usize;
    fn GlobalLock(memory: *mut c_void) -> *mut c_void;
    fn GlobalUnlock(memory: *mut c_void) -> i32;
}

#[cfg(not(feature = "integration-fixtures"))]
#[link(name = "user32")]
extern "system" {
    fn OpenClipboard(owner: *mut c_void) -> i32;
    fn CloseClipboard() -> i32;
    fn GetClipboardData(format: u32) -> *mut c_void;
}

impl CommandProvider for WindowsCommands {
    fn clipboard(&self) -> Option<ClipboardSnapshot> {
        use crate::diagnostics::{self, Level};

        let started = Instant::now();
        let Some(raw) = read_clipboard() else {
            let elapsed = started.elapsed();
            diagnostics::event(
                if elapsed.as_millis() >= 250 {
                    Level::Warning
                } else {
                    Level::Detail
                },
                "command",
                "clipboard_unavailable",
                &[("read_ms", diagnostics::millis(elapsed))],
            );
            return None;
        };
        let read = started.elapsed();
        let conversion_started = Instant::now();
        let converted = if self.sandboxed {
            // 轉換規則有獨立權限，由字庫服務查核。
            crate::broker::clipboard_enabled()
                .then(|| crate::broker::convert_clipboard(&raw).ok())
                .flatten()
        } else {
            convert_clipboard_text(&raw)
        };
        let conversion = conversion_started.elapsed();
        diagnostics::event(
            if (read + conversion).as_millis() >= 250 {
                Level::Warning
            } else {
                Level::Detail
            },
            "command",
            "clipboard",
            &[
                ("sandboxed", i64::from(self.sandboxed)),
                ("conversion_available", i64::from(converted.is_some())),
                ("read_ms", diagnostics::millis(read)),
                ("conversion_ms", diagnostics::millis(conversion)),
            ],
        );
        Some(ClipboardSnapshot::new(raw.into(), converted.map(Arc::from)))
    }

    fn time(&self) -> String {
        let now = crate::win32::LocalTime::now();
        format!("{:02}:{:02}:{:02}", now.hour, now.minute, now.second)
    }

    fn date(&self) -> String {
        let now = crate::win32::LocalTime::now();
        format!("{:02}/{:02}/{:04}", now.day, now.month, now.year)
    }
}

#[cfg(not(feature = "integration-fixtures"))]
struct ClipboardGuard;
#[cfg(not(feature = "integration-fixtures"))]
impl Drop for ClipboardGuard {
    fn drop(&mut self) {
        unsafe { CloseClipboard() };
    }
}

#[cfg(not(feature = "integration-fixtures"))]
struct GlobalGuard(*mut c_void);
#[cfg(not(feature = "integration-fixtures"))]
impl Drop for GlobalGuard {
    fn drop(&mut self) {
        unsafe { GlobalUnlock(self.0) };
    }
}

#[cfg(feature = "integration-fixtures")]
fn read_clipboard() -> Option<String> {
    if std::env::var_os("RUST_IME_TEST_LONG_CLIPBOARD").is_some() {
        Some(("合成測試🙂。".repeat(64) + "\n").repeat(70) + "最後一行。")
    } else {
        Some("合成简体\n第二行🙂".to_owned())
    }
}

#[cfg(not(feature = "integration-fixtures"))]
fn read_clipboard() -> Option<String> {
    if unsafe { OpenClipboard(ptr::null_mut()) } == 0 {
        return None;
    }
    let _clipboard = ClipboardGuard;
    let handle = unsafe { GetClipboardData(13) }; // CF_UNICODETEXT
    if handle.is_null() {
        return None;
    }
    let bytes = unsafe { GlobalSize(handle) };
    if !(2..=64 * 1024 * 1024).contains(&bytes) || bytes % 2 != 0 {
        return None;
    }
    let pointer = unsafe { GlobalLock(handle) }.cast::<u16>();
    if pointer.is_null() {
        return None;
    }
    let _lock = GlobalGuard(handle);
    let words = unsafe { std::slice::from_raw_parts(pointer, bytes / 2) };
    let end = words.iter().position(|word| *word == 0)?;
    if end == 0 {
        return None;
    }
    Some(String::from_utf16_lossy(&words[..end]))
}

struct Converter {
    custom: PhraseTable,
    phrases: PhraseTable,
    characters: HashMap<String, String>,
}

impl Converter {
    fn parse([custom, phrases, characters]: [&str; 3]) -> Result<Self, String> {
        fn table(source: &str, single: bool) -> Result<HashMap<String, String>, String> {
            let entries: HashMap<String, String> =
                serde_json::from_str(source.trim_start_matches('\u{feff}'))
                    .map_err(|error| error.to_string())?;
            if entries.iter().any(|(key, value)| {
                key.is_empty()
                    || value.is_empty()
                    || key.contains('\0')
                    || value.contains('\0')
                    || (single && key.chars().count() != 1)
            }) {
                return Err("簡轉繁 JSON 有無效詞條".into());
            }
            Ok(entries)
        }
        Ok(Self {
            custom: PhraseTable::new(table(custom, false)?),
            phrases: PhraseTable::new(table(phrases, false)?),
            characters: table(characters, true)?,
        })
    }

    fn convert<'a>(&self, text: &'a str) -> Cow<'a, str> {
        replace_prefixes(text, |rest, first| {
            self.custom
                .longest(rest, first)
                .or_else(|| self.phrases.longest(rest, first))
                .or_else(|| {
                    let len = first.len_utf8();
                    self.characters
                        .get(&rest[..len])
                        .map(|value| (len, value.as_str()))
                })
        })
    }
}

fn converter() -> Option<Arc<Converter>> {
    use crate::public_data::{self, Cache};
    static CACHE: Mutex<Cache<Converter, 3>> = Mutex::new(Cache::empty());
    CACHE.lock().unwrap_or_else(|error| error.into_inner()).get(
        public_data::current_directory()?,
        public_data::S2T,
        "s2t",
        Converter::parse,
    )
}

pub(crate) fn convert_clipboard_text(text: &str) -> Option<String> {
    converter().map(|rules| rules.convert(text).into_owned())
}

#[cfg(test)]
mod tests {
    use super::Converter;

    #[test]
    fn converter_uses_longest_match_custom_priority_and_no_second_pass() {
        let converter = Converter::parse([
            r#"{"甲乙":"自訂","保留":"保留"}"#,
            r#"{"甲乙":"二","自訂":"錯","甲乙丙":"最長"}"#,
            r#"{"甲":"一"}"#,
        ])
        .unwrap();
        assert_eq!(
            converter.convert("甲乙丙甲乙甲保留🙂\n"),
            "自訂丙自訂一保留🙂\n"
        );
        assert!(matches!(
            converter.convert("保留🙂\n"),
            std::borrow::Cow::Borrowed("保留🙂\n")
        ));
    }
}
