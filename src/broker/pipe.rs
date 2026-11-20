//! Windows 命名管道、工作階段與存取權杖。
use crate::win32::{wide, GetLastError};
use std::cell::UnsafeCell;
use std::ffi::c_void;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::path::Path;
use std::ptr;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use crate::project_files;

pub(super) type Raw = *mut c_void;
const INVALID: Raw = -1isize as Raw;
const ERROR_FILE_NOT_FOUND: u32 = 2;
const ERROR_ACCESS_DENIED: u32 = 5;
const ERROR_ALREADY_EXISTS: u32 = 183;
const ERROR_PIPE_BUSY: u32 = 231;
const ERROR_PIPE_CONNECTED: u32 = 535;
const ERROR_IO_PENDING: u32 = 997;
const WAIT_TIMEOUT: u32 = 258;
#[repr(C)]
struct SecurityAttributes {
    length: u32,
    descriptor: Raw,
    inherit: i32,
}

#[repr(C)]
struct SidAndAttributes {
    sid: Raw,
    attributes: u32,
}

#[repr(C)]
struct TokenGroups {
    count: u32,
    groups: [SidAndAttributes; 1],
}

#[repr(C)]
struct Overlapped {
    internal: usize,
    high: usize,
    offset: u32,
    offset_high: u32,
    event: Raw,
}

struct OverlappedIo {
    event: OwnedHandle,
    data: UnsafeCell<Overlapped>,
}

impl OverlappedIo {
    fn new() -> Result<Self, String> {
        let event = unsafe { own_handle(CreateEventW(ptr::null(), 1, 0, ptr::null())) }?;
        let data = UnsafeCell::new(Overlapped {
            internal: 0,
            high: 0,
            offset: 0,
            offset_high: 0,
            event: event.as_raw_handle(),
        });
        Ok(Self { event, data })
    }

    fn complete(&self, pipe: &OwnedHandle, pending: bool, timeout: u32) -> Result<usize, String> {
        let mut transferred = 0;
        if pending && unsafe { WaitForSingleObject(self.event.as_raw_handle(), timeout) } != 0 {
            self.cancel(pipe);
            return Err("字庫連線逾時或等待失敗".into());
        }
        if unsafe {
            GetOverlappedResult(pipe.as_raw_handle(), self.data.get(), &mut transferred, 0)
        } == 0
        {
            return Err(last_error());
        }
        Ok(transferred as usize)
    }

    fn cancel(&self, pipe: &OwnedHandle) {
        let mut transferred = 0;
        unsafe {
            CancelIoEx(pipe.as_raw_handle(), self.data.get());
            // Windows 完成取消之前仍會存取 OVERLAPPED 及緩衝區。
            GetOverlappedResult(pipe.as_raw_handle(), self.data.get(), &mut transferred, 1);
        }
    }
}

#[link(name = "kernel32")]
extern "system" {
    fn GetCurrentProcess() -> Raw;
    fn GetCurrentProcessId() -> u32;
    fn ProcessIdToSessionId(pid: u32, session: *mut u32) -> i32;
    fn LocalFree(memory: Raw) -> Raw;
    fn SetLastError(error: u32);
    fn CreateEventW(
        attributes: *const SecurityAttributes,
        manual: i32,
        initial: i32,
        name: *const u16,
    ) -> Raw;
    fn OpenEventW(access: u32, inherit: i32, name: *const u16) -> Raw;
    fn SetEvent(event: Raw) -> i32;
    fn WaitForSingleObject(handle: Raw, milliseconds: u32) -> u32;
    fn WaitForMultipleObjects(count: u32, handles: *const Raw, all: i32, milliseconds: u32) -> u32;
    fn CreateFileW(
        name: *const u16,
        access: u32,
        share: u32,
        security: Raw,
        creation: u32,
        flags: u32,
        template: Raw,
    ) -> Raw;
    fn CreateNamedPipeW(
        name: *const u16,
        open_mode: u32,
        pipe_mode: u32,
        max: u32,
        out_buffer: u32,
        in_buffer: u32,
        timeout: u32,
        security: *const SecurityAttributes,
    ) -> Raw;
    fn ConnectNamedPipe(pipe: Raw, overlapped: *mut Overlapped) -> i32;
    fn ReadFile(
        file: Raw,
        buffer: Raw,
        length: u32,
        read: *mut u32,
        overlapped: *mut Overlapped,
    ) -> i32;
    fn WriteFile(
        file: Raw,
        buffer: *const c_void,
        length: u32,
        written: *mut u32,
        overlapped: *mut Overlapped,
    ) -> i32;
    fn GetOverlappedResult(
        file: Raw,
        overlapped: *mut Overlapped,
        transferred: *mut u32,
        wait: i32,
    ) -> i32;
    fn CancelIoEx(file: Raw, overlapped: *mut Overlapped) -> i32;
    fn WaitNamedPipeW(name: *const u16, timeout: u32) -> i32;
    fn GetNamedPipeClientSessionId(pipe: Raw, session: *mut u32) -> i32;
    fn GetNamedPipeClientProcessId(pipe: Raw, process: *mut u32) -> i32;
    fn GetNamedPipeServerProcessId(pipe: Raw, process: *mut u32) -> i32;
}

#[link(name = "advapi32")]
extern "system" {
    fn OpenProcessToken(process: Raw, access: u32, token: *mut Raw) -> i32;
    fn GetTokenInformation(token: Raw, class: u32, data: Raw, length: u32, needed: *mut u32)
        -> i32;
    fn ConvertSidToStringSidW(sid: Raw, text: *mut *mut u16) -> i32;
    fn ConvertStringSecurityDescriptorToSecurityDescriptorW(
        text: *const u16,
        revision: u32,
        descriptor: *mut Raw,
        size: *mut u32,
    ) -> i32;
}

/// raw 必須是由 Windows 成功傳回且由呼叫者獨佔的 HANDLE，或其失敗值。
pub(super) unsafe fn own_handle(raw: Raw) -> Result<OwnedHandle, String> {
    if raw.is_null() || raw == INVALID {
        Err(last_error())
    } else {
        Ok(OwnedHandle::from_raw_handle(raw))
    }
}

pub(super) struct LocalMemory(Raw);
impl Drop for LocalMemory {
    fn drop(&mut self) {
        unsafe { LocalFree(self.0) };
    }
}

pub(super) fn last_error() -> String {
    win32_error(unsafe { GetLastError() })
}

fn win32_error(error: u32) -> String {
    crate::diagnostics::event(
        crate::diagnostics::Level::Detail,
        "broker_pipe",
        "win32_error",
        &[("win32", i64::from(error))],
    );
    format!("Windows 錯誤 {error}")
}

fn token() -> Result<OwnedHandle, String> {
    let mut token = ptr::null_mut();
    if unsafe { OpenProcessToken(GetCurrentProcess(), 8, &mut token) } == 0 {
        return Err(last_error());
    }
    unsafe { own_handle(token) }
}

pub(super) fn token_information(token: &OwnedHandle, class: u32) -> Result<Vec<usize>, String> {
    let mut size = 0;
    unsafe { GetTokenInformation(token.as_raw_handle(), class, ptr::null_mut(), 0, &mut size) };
    if size == 0 || size > 1024 * 1024 {
        return Err(last_error());
    }
    // usize 對齊 TOKEN_USER 和 TOKEN_GROUPS 內的 SID 指標。
    let mut buffer = vec![0usize; (size as usize).div_ceil(std::mem::size_of::<usize>())];
    if unsafe {
        GetTokenInformation(
            token.as_raw_handle(),
            class,
            buffer.as_mut_ptr().cast(),
            size,
            &mut size,
        )
    } == 0
    {
        return Err(last_error());
    }
    Ok(buffer)
}

pub(super) fn sid_string(sid: Raw) -> Result<String, String> {
    let mut text = ptr::null_mut();
    if unsafe { ConvertSidToStringSidW(sid, &mut text) } == 0 {
        return Err(last_error());
    }
    let memory = LocalMemory(text.cast());
    let mut length = 0;
    unsafe {
        while *text.add(length) != 0 {
            length += 1;
        }
    }
    let value = String::from_utf16(unsafe { std::slice::from_raw_parts(text, length) })
        .map_err(|_| "SID 編碼無效".to_owned())?;
    drop(memory);
    Ok(value)
}

fn current_user_sid() -> Result<String, String> {
    let token = token()?;
    let info = token_information(&token, 1)?;
    sid_string(unsafe { (*(info.as_ptr() as *const SidAndAttributes)).sid })
}

pub(crate) fn current_logon_sid() -> Result<String, String> {
    let token = token()?;
    let info = token_information(&token, 2)?;
    let groups = info.as_ptr() as *const TokenGroups;
    let count = unsafe { (*groups).count as usize };
    let offset = std::mem::offset_of!(TokenGroups, groups);
    if count
        > (info.len() * std::mem::size_of::<usize>() - offset)
            / std::mem::size_of::<SidAndAttributes>()
    {
        return Err("登入群組資料無效".into());
    }
    let first = unsafe { ptr::addr_of!((*groups).groups).cast::<SidAndAttributes>() };
    for index in 0..count {
        let group = unsafe { &*first.add(index) };
        if group.attributes & 0xc000_0000 == 0xc000_0000 {
            return sid_string(group.sid);
        }
    }
    Err("找不到登入工作階段 SID".into())
}

pub(crate) fn is_elevated() -> Result<bool, String> {
    Ok(token_information(&token()?, 20)?[0] as u32 != 0) // TokenElevation
}

pub(crate) fn is_appcontainer() -> Result<bool, String> {
    let token = token()?;
    let mut inside = 0u32;
    let mut size = 0;
    if unsafe {
        GetTokenInformation(
            token.as_raw_handle(),
            29,
            (&mut inside as *mut u32).cast(),
            4,
            &mut size,
        )
    } == 0
    {
        return Err(last_error());
    }
    Ok(inside != 0)
}

pub(super) fn project_id(root: &Path) -> u64 {
    let path = root.to_string_lossy().to_lowercase();
    let path = path.strip_prefix(r"\\?\").unwrap_or(&path);
    let mut hash = 0xcbf29ce484222325u64;
    for byte in path.as_bytes() {
        hash = (hash ^ u64::from(*byte)).wrapping_mul(0x100000001b3);
    }
    hash
}

fn broker_name(root: &Path) -> Result<String, String> {
    let mut session = 0;
    if unsafe { ProcessIdToSessionId(GetCurrentProcessId(), &mut session) } == 0 {
        return Err(last_error());
    }
    let hash = project_id(root);
    Ok(format!(
        "rust-ime-v1-{}-{session}-{hash:016x}",
        current_user_sid()?
    ))
}

pub(super) fn pipe_name(root: &Path) -> Result<Vec<u16>, String> {
    Ok(wide(&format!(r"\\.\pipe\{}", broker_name(root)?)))
}

pub(super) fn client_pipe_name() -> Result<&'static [u16], String> {
    static NAME: OnceLock<Result<Vec<u16>, String>> = OnceLock::new();
    NAME.get_or_init(|| pipe_name(&project_files::dll_root()?))
        .as_deref()
        .map_err(Clone::clone)
}

fn deadline_ms(deadline: Instant) -> u32 {
    deadline
        .saturating_duration_since(Instant::now())
        .as_millis()
        .min(u128::from(u32::MAX - 1)) as u32
}

fn transfer_all(
    pipe: &OwnedHandle,
    length: usize,
    deadline: Instant,
    mut transfer: impl FnMut(usize, u32, *mut Overlapped) -> i32,
) -> Result<(), String> {
    let mut offset = 0;
    while offset < length {
        let operation = OverlappedIo::new()?;
        let size = (length - offset).min(u32::MAX as usize) as u32;
        let pending = transfer(offset, size, operation.data.get()) == 0;
        if pending && unsafe { GetLastError() } != ERROR_IO_PENDING {
            return Err(last_error());
        }
        let timeout = if pending { deadline_ms(deadline) } else { 0 };
        let transferred = operation.complete(pipe, pending, timeout)?;
        if transferred == 0 {
            return Err("字庫連線已關閉".into());
        }
        offset += transferred;
    }
    Ok(())
}

pub(super) fn read_all(
    pipe: &OwnedHandle,
    buffer: &mut [u8],
    deadline: Instant,
) -> Result<(), String> {
    transfer_all(
        pipe,
        buffer.len(),
        deadline,
        |offset, size, operation| unsafe {
            ReadFile(
                pipe.as_raw_handle(),
                buffer.as_mut_ptr().add(offset).cast(),
                size,
                ptr::null_mut(),
                operation,
            )
        },
    )
}

pub(super) fn write_all(
    pipe: &OwnedHandle,
    buffer: &[u8],
    deadline: Instant,
) -> Result<(), String> {
    transfer_all(
        pipe,
        buffer.len(),
        deadline,
        |offset, size, operation| unsafe {
            WriteFile(
                pipe.as_raw_handle(),
                buffer.as_ptr().add(offset).cast(),
                size,
                ptr::null_mut(),
                operation,
            )
        },
    )
}

pub(super) fn make_security() -> Result<LocalMemory, String> {
    // 部分 AppContainer token 沒有登入 SID，因此也須授權目前使用者連線；
    // 此權限與受限 SID 權限都不含 FILE_CREATE_PIPE_INSTANCE。
    // 連線後另行核對工作階段，並由 CreateNamedPipeW 拒絕遠端連線。
    let sddl = format!(
        "D:(A;;FA;;;SY)(A;;FA;;;{})(A;;0x00120083;;;{})(A;;0x00120083;;;AC)(A;;0x00120083;;;S-1-15-2-2)S:(ML;;NW;;;LW)",
        current_logon_sid()?,
        current_user_sid()?
    );
    security_descriptor(&sddl)
}

fn security_descriptor(sddl: &str) -> Result<LocalMemory, String> {
    let sddl = wide(sddl);
    let mut descriptor = ptr::null_mut();
    if unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            sddl.as_ptr(),
            1,
            &mut descriptor,
            ptr::null_mut(),
        )
    } == 0
    {
        return Err(last_error());
    }
    Ok(LocalMemory(descriptor))
}

fn stop_event_name(root: &Path) -> Result<Vec<u16>, String> {
    Ok(wide(&format!(r"Local\{}-stop", broker_name(root)?)))
}

pub(super) fn create_stop_event(root: &Path) -> Result<OwnedHandle, String> {
    let name = stop_event_name(root)?;
    // 同一帳戶的一般程序可停止升權後啟動的字庫服務；低完整性程序不能寫入此事件。
    let security = security_descriptor(&format!(
        "D:(A;;GA;;;SY)(A;;GA;;;{})S:(ML;;NW;;;ME)",
        current_user_sid()?
    ))?;
    let attributes = SecurityAttributes {
        length: std::mem::size_of::<SecurityAttributes>() as u32,
        descriptor: security.0,
        inherit: 0,
    };
    let raw = unsafe {
        SetLastError(0);
        CreateEventW(&attributes, 1, 0, name.as_ptr())
    };
    let error = unsafe { GetLastError() };
    if raw.is_null() {
        return Err(win32_error(error));
    }
    let event = unsafe { OwnedHandle::from_raw_handle(raw) };
    if error == ERROR_ALREADY_EXISTS {
        return Err("字庫服務的停止事件已被佔用".into());
    }
    Ok(event)
}

pub(super) fn request_stop(root: &Path) -> Result<(), String> {
    let name = stop_event_name(root)?;
    let raw = unsafe { OpenEventW(2, 0, name.as_ptr()) }; // EVENT_MODIFY_STATE
    if raw.is_null() {
        let error = unsafe { GetLastError() };
        return if error == ERROR_FILE_NOT_FOUND {
            Ok(())
        } else {
            Err(win32_error(error))
        };
    }
    let event = unsafe { OwnedHandle::from_raw_handle(raw) };
    if unsafe { SetEvent(event.as_raw_handle()) } == 0 {
        return Err(last_error());
    }
    Ok(())
}

pub(super) fn create_pipe(
    name: &[u16],
    security: &LocalMemory,
    first: bool,
    retry_until: Option<Instant>,
) -> Result<OwnedHandle, String> {
    let attributes = SecurityAttributes {
        length: std::mem::size_of::<SecurityAttributes>() as u32,
        descriptor: security.0,
        inherit: 0,
    };
    let flags = 0x0000_0003 | 0x4000_0000 | if first { 0x0008_0000 } else { 0 };
    loop {
        let raw = unsafe {
            CreateNamedPipeW(
                name.as_ptr(),
                flags,
                0x0000_0008,
                12,
                65536,
                4096,
                0,
                &attributes,
            )
        };
        if raw != INVALID {
            return unsafe { own_handle(raw) };
        }
        let error = unsafe { GetLastError() };
        if error != ERROR_ACCESS_DENIED
            || !retry_until.is_some_and(|deadline| Instant::now() < deadline)
        {
            return Err(win32_error(error));
        }
        std::thread::sleep(Duration::from_millis(40));
    }
}

pub(super) fn stopped(event: &OwnedHandle) -> Result<bool, String> {
    match unsafe { WaitForSingleObject(event.as_raw_handle(), 0) } {
        0 => Ok(true),
        WAIT_TIMEOUT => Ok(false),
        _ => Err(last_error()),
    }
}

pub(super) fn connect(pipe: &OwnedHandle, stop: Option<&OwnedHandle>) -> Result<bool, String> {
    let operation = OverlappedIo::new()?;
    if unsafe { ConnectNamedPipe(pipe.as_raw_handle(), operation.data.get()) } != 0 {
        return Ok(true);
    }
    match unsafe { GetLastError() } {
        ERROR_PIPE_CONNECTED => Ok(true),
        ERROR_IO_PENDING => {
            let Some(stop) = stop else {
                return operation.complete(pipe, true, u32::MAX).map(|_| true);
            };
            let handles = [stop.as_raw_handle(), operation.event.as_raw_handle()];
            match unsafe { WaitForMultipleObjects(2, handles.as_ptr(), 0, u32::MAX) } {
                0 => {
                    operation.cancel(pipe);
                    Ok(false)
                }
                1 => operation.complete(pipe, false, 0).map(|_| true),
                _ => {
                    let error = last_error();
                    operation.cancel(pipe);
                    Err(error)
                }
            }
        }
        _ => Err(last_error()),
    }
}

pub(super) fn server_process(pipe: &OwnedHandle) -> Result<u32, String> {
    let mut pid = 0;
    if unsafe { GetNamedPipeServerProcessId(pipe.as_raw_handle(), &mut pid) } == 0 {
        return Err(last_error());
    }
    Ok(pid)
}

pub(super) fn open_client(name: &[u16], deadline: Instant) -> Result<OwnedHandle, String> {
    loop {
        let raw = unsafe {
            CreateFileW(
                name.as_ptr(),
                0x0012_0003,
                0,
                ptr::null_mut(),
                3,
                0x4011_0000, // FILE_FLAG_OVERLAPPED | SECURITY_SQOS_PRESENT | SECURITY_IDENTIFICATION
                ptr::null_mut(),
            )
        };
        if raw != INVALID {
            return unsafe { own_handle(raw) };
        }
        if unsafe { GetLastError() } != ERROR_PIPE_BUSY || deadline_ms(deadline) == 0 {
            return Err(last_error());
        }
        unsafe { WaitNamedPipeW(name.as_ptr(), deadline_ms(deadline).min(30)) };
    }
}

pub(super) fn open_anchor(name: &[u16]) -> Result<OwnedHandle, String> {
    unsafe {
        own_handle(CreateFileW(
            name.as_ptr(),
            0x0012_0003,
            0,
            ptr::null_mut(),
            3,
            0,
            ptr::null_mut(),
        ))
    }
}

pub(super) fn client_process(pipe: &OwnedHandle) -> Result<u32, String> {
    let mut client_session = 0;
    let mut server_session = 0;
    let mut client_process = 0;
    if unsafe { GetNamedPipeClientSessionId(pipe.as_raw_handle(), &mut client_session) } == 0
        || unsafe { GetNamedPipeClientProcessId(pipe.as_raw_handle(), &mut client_process) } == 0
        || unsafe { ProcessIdToSessionId(GetCurrentProcessId(), &mut server_session) } == 0
        || client_session != server_session
    {
        return Err("連線並非來自目前登入工作階段".into());
    }
    Ok(client_process)
}
