//! 管理登錄控制代碼，並保留呼叫者指定的 x64／x86 登錄檢視。
use super::{wide, wide_os};
use std::ffi::{c_void, OsStr};
use std::ptr;

type Raw = *mut c_void;
pub(crate) const HKCU: Raw = 0x8000_0001u32 as i32 as isize as Raw;
pub(crate) const HKLM: Raw = 0x8000_0002u32 as i32 as isize as Raw;

#[link(name = "advapi32")]
extern "system" {
    fn RegCreateKeyExW(
        parent: Raw,
        path: *const u16,
        reserved: u32,
        class: *mut u16,
        options: u32,
        access: u32,
        security: Raw,
        key: *mut Raw,
        disposition: *mut u32,
    ) -> i32;
    fn RegCloseKey(key: Raw) -> i32;
    pub(crate) fn RegGetValueW(
        key: Raw,
        subkey: *const u16,
        name: *const u16,
        flags: u32,
        kind: *mut u32,
        data: Raw,
        size: *mut u32,
    ) -> i32;
    fn RegSetValueExW(
        key: Raw,
        name: *const u16,
        reserved: u32,
        kind: u32,
        data: *const u8,
        size: u32,
    ) -> i32;
    pub(crate) fn RegDeleteKeyExW(key: Raw, subkey: *const u16, access: u32, reserved: u32) -> i32;
    pub(crate) fn RegDeleteTreeW(key: Raw, subkey: *const u16) -> i32;
}

pub(crate) struct Key(pub(crate) Raw);

// HKEY 可跨執行緒使用。Key 獨佔一次 RegCloseKey；服務端以 Mutex 協調模式讀寫。
unsafe impl Send for Key {}

impl Drop for Key {
    fn drop(&mut self) {
        unsafe { RegCloseKey(self.0) };
    }
}

impl Key {
    pub(crate) fn dword(&self, name: &str) -> Option<u32> {
        unsafe { read_dword(self.0, ptr::null(), name) }
    }

    pub(crate) fn set_dword(&self, name: &str, value: u32) -> Result<(), String> {
        let rc = unsafe {
            RegSetValueExW(
                self.0,
                wide(name).as_ptr(),
                0,
                4,
                (&value as *const u32).cast(),
                4,
            )
        };
        if rc == 0 {
            Ok(())
        } else {
            Err(format!("無法寫入登錄設定（{rc}）"))
        }
    }

    pub(crate) fn set_string(
        &self,
        name: Option<&str>,
        value: impl AsRef<OsStr>,
    ) -> Result<(), i32> {
        let name = name.map(wide);
        let value = wide_os(value.as_ref());
        let rc = unsafe {
            RegSetValueExW(
                self.0,
                name.as_ref().map_or(ptr::null(), |name| name.as_ptr()),
                0,
                1,
                value.as_ptr().cast(),
                (value.len() * 2) as u32,
            )
        };
        if rc == 0 {
            Ok(())
        } else {
            Err(rc)
        }
    }

    pub(crate) unsafe fn create(
        parent: Raw,
        path: &str,
        options: u32,
        access: u32,
    ) -> Result<(Self, u32), i32> {
        let mut key = ptr::null_mut();
        let mut disposition = 0;
        let rc = RegCreateKeyExW(
            parent,
            wide(path).as_ptr(),
            0,
            ptr::null_mut(),
            options,
            access,
            ptr::null_mut(),
            &mut key,
            &mut disposition,
        );
        if rc == 0 {
            Ok((Self(key), disposition))
        } else {
            Err(rc)
        }
    }
}

/// subkey 可為空指標，或指向有效、以 NUL 結束的 UTF-16 路徑。
pub(crate) unsafe fn read_dword(key: Raw, subkey: *const u16, name: &str) -> Option<u32> {
    let mut value = 0u32;
    let mut size = 4;
    let rc = RegGetValueW(
        key,
        subkey,
        wide(name).as_ptr(),
        0x10,
        ptr::null_mut(),
        (&mut value as *mut u32).cast(),
        &mut size,
    );
    (rc == 0 && size == 4).then_some(value)
}
