//! Windows 事件記錄。呼叫端只提供固定事件名稱及數值欄位。
use crate::win32::{wide, GetLastError};
use std::collections::BTreeMap;
use std::ffi::c_void;
use std::os::windows::ffi::OsStrExt;
use std::ptr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use crate::tsf::ffi::{Guid, HResult};

const PROVIDER: Guid = Guid::new(
    0x9d7644e6,
    0x4f13,
    0x49a1,
    [0x91, 0xcf, 0x2f, 0x64, 0xc8, 0x0c, 0x24, 0xb7],
);
pub(crate) const RELEASE: &str = match option_env!("RUST_IME_RELEASE") {
    Some(value) => value,
    None => include_str!("../release-version.txt"),
};
static HANDLE: AtomicU64 = AtomicU64::new(0);
static REGISTERED: OnceLock<()> = OnceLock::new();

#[repr(C)]
struct Descriptor {
    id: u16,
    version: u8,
    channel: u8,
    level: u8,
    opcode: u8,
    task: u16,
    keyword: u64,
}

#[repr(C)]
struct Data {
    ptr: u64,
    size: u32,
    reserved: u32,
}

#[link(name = "advapi32")]
extern "system" {
    fn EventRegister(
        provider: *const Guid,
        callback: *const c_void,
        context: *const c_void,
        handle: *mut u64,
    ) -> u32;
    fn EventUnregister(handle: u64) -> u32;
    fn EventEnabled(handle: u64, event: *const Descriptor) -> u8;
    fn EventWrite(handle: u64, event: *const Descriptor, count: u32, data: *const Data) -> u32;
}

#[link(name = "kernel32")]
extern "system" {
    fn SetLastError(error: u32);
}

#[derive(Clone, Copy)]
pub(crate) enum Level {
    Error,
    Warning,
    Info,
    Detail,
}

impl Level {
    fn descriptor(self) -> Descriptor {
        // 通道和關鍵字與 assets/oxide-ime-events.man 及 mc.exe 生成的值一致。
        let (id, level) = match self {
            Self::Error => (1, 2),
            Self::Warning => (2, 3),
            Self::Info => (3, 4),
            Self::Detail => (4, 4),
        };
        let detail = matches!(self, Self::Detail);
        Descriptor {
            id,
            version: 0,
            channel: if detail { 17 } else { 16 },
            level,
            opcode: 0,
            task: 0,
            keyword: if detail { 1 << 62 } else { 1 << 63 },
        }
    }
}

fn with_provider<T>(action: impl FnOnce(u64) -> T) -> T {
    // Windows 輸入宿主可能在呼叫前保留 Win32 錯誤碼。
    let last_error = unsafe { GetLastError() };
    REGISTERED.get_or_init(|| {
        // 不註冊回呼，也不在 DllMain 啟動事件供應者。
        let mut handle = 0;
        unsafe { EventRegister(&PROVIDER, ptr::null(), ptr::null(), &mut handle) };
        HANDLE.store(handle, Ordering::Release);
    });
    let result = action(HANDLE.load(Ordering::Acquire));
    unsafe { SetLastError(last_error) };
    result
}

pub(crate) fn shutdown() {
    let handle = HANDLE.swap(0, Ordering::AcqRel);
    if handle != 0 {
        unsafe { EventUnregister(handle) };
    }
}

pub(crate) fn detailed() -> bool {
    with_provider(|handle| unsafe { EventEnabled(handle, &Level::Detail.descriptor()) != 0 })
}

// 選單以事件檢視器的通道設定為準，獨立的 ETW 追蹤也能啟用 EventEnabled。
pub(crate) fn detail_channel_enabled() -> bool {
    #[repr(C)]
    struct Variant {
        value: u64,
        count: u32,
        kind: u32,
    }
    type Handle = *mut c_void;
    #[link(name = "wevtapi")]
    extern "system" {
        fn EvtOpenChannelConfig(session: Handle, path: *const u16, flags: u32) -> Handle;
        fn EvtGetChannelConfigProperty(
            config: Handle,
            property: u32,
            flags: u32,
            size: u32,
            value: *mut Variant,
            used: *mut u32,
        ) -> i32;
        fn EvtClose(handle: Handle) -> i32;
    }
    const ENABLED: u32 = 0;
    const BOOLEAN: u32 = 13;
    unsafe {
        let config = EvtOpenChannelConfig(ptr::null_mut(), wide("OxideIme/Diagnostic").as_ptr(), 0);
        if config.is_null() {
            return false;
        }
        let mut value = Variant {
            value: 0,
            count: 0,
            kind: 0,
        };
        let mut used = 0;
        let ok = EvtGetChannelConfigProperty(
            config,
            ENABLED,
            0,
            std::mem::size_of::<Variant>() as u32,
            &mut value,
            &mut used,
        );
        EvtClose(config);
        ok != 0 && value.kind == BOOLEAN && value.value as u32 != 0
    }
}

fn host() -> &'static [u16] {
    static HOST: OnceLock<Vec<u16>> = OnceLock::new();
    HOST.get_or_init(|| {
        std::env::current_exe()
            .ok()
            .and_then(|path| Some(path.file_name()?.encode_wide().take(96).collect::<Vec<_>>()))
            .unwrap_or_default()
            .into_iter()
            .chain([0])
            .collect()
    })
}

pub(crate) fn event(
    level: Level,
    component: &'static str,
    operation: &'static str,
    numbers: &[(&'static str, i64)],
) {
    with_provider(|handle| {
        let descriptor = level.descriptor();
        if unsafe { EventEnabled(handle, &descriptor) } == 0 {
            return;
        }
        // 只接受固定欄位名稱及數值，不提供記錄文字、路徑或任意錯誤訊息的接口。
        let mut fields = numbers.iter().copied().collect::<BTreeMap<_, _>>();
        fields.insert("pointer_bits", i64::from(usize::BITS));
        let details = serde_json::to_string(&fields);
        let details = wide(details.as_deref().unwrap_or("{}"));
        let release = wide(RELEASE.trim());
        let component = wide(component);
        let operation = wide(operation);
        let payload =
            [release.as_slice(), host(), &component, &operation, &details].map(|text| Data {
                ptr: text.as_ptr() as usize as u64,
                size: std::mem::size_of_val(text) as u32,
                reserved: 0,
            });
        unsafe { EventWrite(handle, &descriptor, payload.len() as u32, payload.as_ptr()) };
    });
}

pub(crate) fn hresult(hr: HResult) -> i64 {
    i64::from(hr as u32)
}

pub(crate) fn result(component: &'static str, operation: &'static str, hr: HResult) {
    let level = if hr < 0 { Level::Error } else { Level::Info };
    event(level, component, operation, &[("hresult", hresult(hr))]);
}

pub(crate) fn millis(elapsed: Duration) -> i64 {
    i64::try_from(elapsed.as_millis()).unwrap_or(i64::MAX)
}

/// 壓低持續失敗時的記錄頻率，避免循環覆寫最近的其他事件。
pub(crate) struct WarningLimit(Mutex<Option<Instant>>);

impl WarningLimit {
    pub(crate) const fn new() -> Self {
        Self(Mutex::new(None))
    }

    pub(crate) fn ready(&self) -> bool {
        let mut last = self.0.lock().unwrap_or_else(|error| error.into_inner());
        let now = Instant::now();
        if last.is_some_and(|previous| now.duration_since(previous) < Duration::from_secs(5)) {
            return false;
        }
        *last = Some(now);
        true
    }
}
