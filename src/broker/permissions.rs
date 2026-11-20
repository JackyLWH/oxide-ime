//! 驗證沙盒身分，並沿用已保存的私人剪貼簿轉換權限。
use super::pipe::{last_error, own_handle, sid_string, token_information, Raw};
use crate::win32::{
    registry::{self, HKCU},
    wide,
};
use std::os::windows::io::{AsRawHandle, OwnedHandle};
use std::ptr;

#[link(name = "advapi32")]
extern "system" {
    fn ImpersonateNamedPipeClient(pipe: Raw) -> i32;
    fn OpenThreadToken(thread: Raw, access: u32, open_as_self: i32, token: *mut Raw) -> i32;
    fn RevertToSelf() -> i32;
    #[cfg(feature = "integration-fixtures")]
    fn RegSetKeyValueW(
        key: Raw,
        subkey: *const u16,
        name: *const u16,
        kind: u32,
        data: *const u32,
        size: u32,
    ) -> i32;
}

#[link(name = "kernel32")]
extern "system" {
    fn GetCurrentThread() -> Raw;
}

struct Impersonation;

impl Drop for Impersonation {
    fn drop(&mut self) {
        // 此執行緒若不能還原，不得再以錯誤身分存取資料。
        if unsafe { RevertToSelf() } == 0 {
            std::process::abort();
        }
    }
}

pub(super) fn client_sid(pipe: &OwnedHandle) -> Result<Option<String>, String> {
    if unsafe { ImpersonateNamedPipeClient(pipe.as_raw_handle()) } == 0 {
        return Err(last_error());
    }
    let identity = Impersonation;
    let mut raw = ptr::null_mut();
    if unsafe { OpenThreadToken(GetCurrentThread(), 8, 1, &mut raw) } == 0 {
        return Err(last_error());
    }
    let token = unsafe { own_handle(raw) }?;
    drop(identity);
    let info = token_information(&token, 31)?; // TokenAppContainerSid
    match info[0] as Raw {
        sid if sid.is_null() => Ok(None),
        sid => sid_string(sid).map(Some),
    }
}

pub(super) struct Permissions {
    key: Vec<u16>,
}

impl Permissions {
    pub(super) fn new(project_id: u64) -> Self {
        Self {
            key: wide(&format!(
                r"Software\RustIme\{project_id:016x}\SandboxDictionaries"
            )),
        }
    }

    pub(super) fn enabled(&self, sid: &str) -> bool {
        unsafe { registry::read_dword(HKCU, self.key.as_ptr(), sid) == Some(1) }
    }

    #[cfg(feature = "integration-fixtures")]
    pub(super) fn write(&self, sid: &str, enabled: bool) -> Result<(), String> {
        let value = wide(sid);
        let data = u32::from(enabled);
        let rc = unsafe { RegSetKeyValueW(HKCU, self.key.as_ptr(), value.as_ptr(), 4, &data, 4) };
        if rc == 0 {
            Ok(())
        } else {
            Err(format!("無法保存私人詞典授權（{rc}）"))
        }
    }
}
