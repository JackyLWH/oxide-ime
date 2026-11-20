//! Windows 載入、註冊與卸載文字服務所用的 DLL 入口。
use std::ffi::c_void;
use std::sync::atomic::Ordering;

use super::ffi::{self, Guid, HResult, CLASS_E_CLASSNOTAVAILABLE, S_FALSE, S_OK};
use super::{com, registration, CLSID, MODULE, OBJECTS};
use crate::diagnostics;

#[no_mangle]
pub extern "system" fn DllMain(module: *mut c_void, reason: u32, reserved: *mut c_void) -> i32 {
    if reason == 1 {
        MODULE.store(module, Ordering::Relaxed);
    } else if reason == 0 && reserved.is_null() {
        diagnostics::shutdown();
    }
    1
}

#[no_mangle]
/// 提供 Windows COM 載入文字服務所需的類別工廠。
///
/// # Safety
/// 非空輸入指標必須指向有效的 GUID；`out` 必須可寫入一個 COM 指標。
pub unsafe extern "system" fn DllGetClassObject(
    clsid: *const Guid,
    iid: *const Guid,
    out: *mut *mut c_void,
) -> HResult {
    if out.is_null() {
        return ffi::E_POINTER;
    }
    *out = std::ptr::null_mut();
    if clsid.is_null() || iid.is_null() {
        return ffi::E_POINTER;
    }
    if *clsid != CLSID {
        return CLASS_E_CLASSNOTAVAILABLE;
    }
    let result = com::create_factory(iid, out);
    if result < 0 {
        diagnostics::result("com", "create_factory", result);
    }
    result
}

#[no_mangle]
pub extern "system" fn DllCanUnloadNow() -> HResult {
    if OBJECTS.load(Ordering::SeqCst) == 0 {
        S_OK
    } else {
        S_FALSE
    }
}

#[no_mangle]
pub extern "system" fn DllRegisterServer() -> HResult {
    let result = unsafe { registration::register() };
    diagnostics::result("registration", "register", result);
    result
}

#[no_mangle]
pub extern "system" fn DllUnregisterServer() -> HResult {
    let result = unsafe { registration::unregister() };
    diagnostics::result("registration", "unregister", result);
    result
}

#[no_mangle]
/// 更新已註冊文字服務的系統匣、UIElement、沉浸式或顯示屬性類別。
///
/// # Safety
/// `command` 為空指標或指向有效的 NUL 結尾 UTF-16 字串。
pub unsafe extern "system" fn DllInstall(install: i32, command: *const u16) -> HResult {
    unsafe {
        let mut length = 0usize;
        if !command.is_null() {
            while length < 32 && *command.add(length) != 0 {
                length += 1;
            }
            if length == 32 {
                return ffi::E_INVALIDARG;
            }
        }
        let argument = if command.is_null() {
            String::new()
        } else {
            String::from_utf16_lossy(std::slice::from_raw_parts(command, length))
        };
        let category = match argument.to_ascii_lowercase().as_str() {
            "" | "tray" => &ffi::GUID_TFCAT_TIPCAP_SYSTRAYSUPPORT,
            "uielement" => &ffi::GUID_TFCAT_TIPCAP_UIELEMENTENABLED,
            "immersive" => &ffi::GUID_TFCAT_TIPCAP_IMMERSIVESUPPORT,
            "displayattribute" => &ffi::GUID_TFCAT_DISPLAYATTRIBUTEPROVIDER,
            _ => return ffi::E_INVALIDARG,
        };
        let result = registration::install_capability(install != 0, category);
        diagnostics::event(
            if result < 0 {
                diagnostics::Level::Error
            } else {
                diagnostics::Level::Info
            },
            "registration",
            "capability",
            &[
                ("hresult", diagnostics::hresult(result)),
                ("enable", i64::from(install != 0)),
                ("category", i64::from(category.data1)),
            ],
        );
        result
    }
}

#[cfg(feature = "integration-fixtures")]
#[no_mangle]
pub extern "system" fn RustImeSyntheticClipboardFixture() -> i32 {
    1
}
