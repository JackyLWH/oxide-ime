//! 透過 Windows COM、TSF 與登錄 API 註冊；失敗時回復新建的項目。
use crate::win32::{
    registry::{self, Key, HKLM},
    wide,
};
use std::ptr;

use super::ffi::{self, CategoryMgrVtbl, ComPtr, HResult, ProfileMgrVtbl, Raw, ThreadManagerVtbl};
use super::{CLSID, LANGUAGE, PROFILE};
use crate::project_files;

const CLASS_KEY: &str = r"SOFTWARE\Classes\CLSID\{4BD0700E-5DD1-4310-A9D2-F3D579D54730}";
const DESCRIPTION: &str = crate::PRODUCT_NAME;
const KEY_WRITE_VIEW: u32 = 0x20006
    | if cfg!(target_arch = "x86") {
        0x200
    } else {
        0x100
    };

fn win32_error(code: i32) -> HResult {
    if code <= 0 {
        code
    } else {
        (0x80070000u32 | code as u32) as i32
    }
}

unsafe fn create_key(parent: Raw, path: &str) -> Result<(Key, u32), HResult> {
    Key::create(parent, path, 0, KEY_WRITE_VIEW).map_err(win32_error)
}

struct ComApartment(bool);
impl ComApartment {
    unsafe fn enter() -> Result<Self, HResult> {
        let hr = ffi::CoInitializeEx(ptr::null_mut(), 2); // STA；regsvr32 可能已初始化 COM。
        if hr == ffi::RPC_E_CHANGED_MODE {
            Ok(Self(false))
        } else if hr < 0 {
            Err(hr)
        } else {
            Ok(Self(true))
        }
    }
}
impl Drop for ComApartment {
    fn drop(&mut self) {
        if self.0 {
            unsafe {
                ffi::CoUninitialize();
            }
        }
    }
}

unsafe fn profiles() -> Result<ComPtr, HResult> {
    ffi::create_instance(
        &ffi::CLSID_TF_InputProcessorProfiles,
        &ffi::IID_ITfInputProcessorProfileMgr,
    )
}

pub(crate) fn profile_is_active() -> bool {
    unsafe {
        let Ok(_apartment) = ComApartment::enter() else {
            return false;
        };
        let Ok(manager) = profiles() else {
            return false;
        };
        let manager = manager.as_raw();
        let mut active = std::mem::MaybeUninit::<ffi::InputProcessorProfile>::uninit();
        let hr = (ffi::vtable::<ProfileMgrVtbl>(manager).get_active_profile)(
            manager,
            &ffi::GUID_TFCAT_TIP_KEYBOARD,
            active.as_mut_ptr(),
        );
        if hr != ffi::S_OK {
            return false;
        }
        let active = active.assume_init();
        active.clsid == CLSID && active.profile == PROFILE
    }
}

pub(crate) fn change_profile(activate: bool) -> Result<(), HResult> {
    const INPUT_PROCESSOR: u32 = 1;
    const FOR_SESSION: u32 = 0x2000_0000;
    const ENABLE_PROFILE: u32 = 1;
    const IGNORE_INPUT_LANGUAGE: u32 = 4;
    const FREE_UNUSED_LIBRARIES: u32 = 1;
    unsafe {
        let _apartment = ComApartment::enter()?;
        let thread = ffi::create_instance(&ffi::CLSID_TF_ThreadMgr, &ffi::IID_ITfThreadMgr)?;
        let thread_vtbl = ffi::vtable::<ThreadManagerVtbl>(thread.as_raw());
        let mut client = 0;
        // 背景程序也必須先啟用自己的 TSF 執行緒，才能切換桌面的輸入法。
        let hr = (thread_vtbl.activate)(thread.as_raw(), &mut client);
        if hr < 0 {
            return Err(hr);
        }
        let manager = match profiles() {
            Ok(manager) => manager,
            Err(hr) => {
                (thread_vtbl.deactivate)(thread.as_raw());
                return Err(hr);
            }
        };
        let raw = manager.as_raw();
        let vtbl = ffi::vtable::<ProfileMgrVtbl>(raw);
        let result = if activate {
            let hr = (vtbl.activate_profile)(
                raw,
                INPUT_PROCESSOR,
                LANGUAGE,
                &CLSID,
                &PROFILE,
                ptr::null_mut(),
                FOR_SESSION | ENABLE_PROFILE | IGNORE_INPUT_LANGUAGE,
            );
            crate::diagnostics::result("profile", "restart_activate", hr);
            hr
        } else {
            let hr = (vtbl.deactivate_profile)(
                raw,
                INPUT_PROCESSOR,
                LANGUAGE,
                &CLSID,
                &PROFILE,
                ptr::null_mut(),
                FOR_SESSION,
            );
            crate::diagnostics::result("profile", "restart_deactivate", hr);
            // 讓 TSF 放開已停用的服務。由另一個程序重新啟用，讓 COM 重建介面。
            let released = (vtbl.release_processor)(raw, &CLSID, FREE_UNUSED_LIBRARIES);
            crate::diagnostics::result("profile", "restart_release", released);
            if hr < 0 {
                hr
            } else {
                released
            }
        };
        (thread_vtbl.deactivate)(thread.as_raw());
        if result < 0 {
            Err(result)
        } else {
            Ok(())
        }
    }
}
unsafe fn categories() -> Result<ComPtr, HResult> {
    ffi::create_instance(&ffi::CLSID_TF_CategoryMgr, &ffi::IID_ITfCategoryMgr)
}

unsafe fn profile_categories() -> [&'static ffi::Guid; 5] {
    [
        &ffi::GUID_TFCAT_TIP_KEYBOARD,
        &ffi::GUID_TFCAT_TIPCAP_SYSTRAYSUPPORT,
        &ffi::GUID_TFCAT_TIPCAP_UIELEMENTENABLED,
        &ffi::GUID_TFCAT_TIPCAP_IMMERSIVESUPPORT,
        &ffi::GUID_TFCAT_DISPLAYATTRIBUTEPROVIDER,
    ]
}

// x86 與 x64 共用 TSF 設定檔，只需在 32 位元 COM 登錄檢視註冊各自的 DLL。
unsafe fn register_class_only(path: &str) -> Result<(), HResult> {
    let (class, class_disposition) = create_key(HKLM, CLASS_KEY)?;
    let mut server_created = false;
    let result = (|| {
        if class_disposition == 1 {
            class.set_string(None, DESCRIPTION).map_err(win32_error)?;
        }
        let (server, disposition) = create_key(class.0, "InprocServer32")?;
        if disposition != 1 {
            return Err(win32_error(183));
        }
        server_created = true;
        server.set_string(None, path).map_err(win32_error)?;
        server
            .set_string(Some("ThreadingModel"), "Apartment")
            .map_err(win32_error)
    })();
    if result.is_err() {
        drop(class);
        if class_disposition == 1 {
            let _ = registry::RegDeleteTreeW(HKLM, wide(CLASS_KEY).as_ptr());
        } else if server_created {
            let server = wide(&format!(r"{CLASS_KEY}\InprocServer32"));
            let _ = registry::RegDeleteKeyExW(HKLM, server.as_ptr(), 0x200, 0);
        }
    }
    result
}

unsafe fn register_inner(path: &str) -> Result<(), HResult> {
    let _apartment = ComApartment::enter()?;
    let profile_mgr = profiles()?;
    let category_mgr = categories()?;

    let (class, disposition) = create_key(HKLM, CLASS_KEY)?;
    if disposition != 1 {
        // 不覆蓋已安裝的版本；先解除註冊才能重新安裝。
        return Err(win32_error(183));
    }
    let mut profile_registered = false;
    let mut categories_registered = Vec::new();
    let result = (|| {
        class.set_string(None, DESCRIPTION).map_err(win32_error)?;
        let (server, _) = create_key(class.0, "InprocServer32")?;
        server.set_string(None, path).map_err(win32_error)?;
        server
            .set_string(Some("ThreadingModel"), "Apartment")
            .map_err(win32_error)?;

        let desc: Vec<u16> = DESCRIPTION.encode_utf16().collect();
        let icon: Vec<u16> = path.encode_utf16().collect();
        let hr = (ffi::vtable::<ProfileMgrVtbl>(profile_mgr.as_raw()).register_profile)(
            profile_mgr.as_raw(),
            &CLSID,
            LANGUAGE,
            &PROFILE,
            desc.as_ptr(),
            desc.len() as u32,
            icon.as_ptr(),
            icon.len() as u32,
            0,
            ptr::null_mut(),
            0,
            0,
            0,
        );
        if hr < 0 {
            return Err(hr);
        }
        profile_registered = true;

        for category in profile_categories() {
            let hr = (ffi::vtable::<CategoryMgrVtbl>(category_mgr.as_raw()).register_category)(
                category_mgr.as_raw(),
                &CLSID,
                category,
                &CLSID,
            );
            if hr < 0 {
                return Err(hr);
            }
            categories_registered.push(category);
        }
        Ok(())
    })();

    if result.is_err() {
        for category in categories_registered.into_iter().rev() {
            let _ = (ffi::vtable::<CategoryMgrVtbl>(category_mgr.as_raw()).unregister_category)(
                category_mgr.as_raw(),
                &CLSID,
                category,
                &CLSID,
            );
        }
        if profile_registered {
            let _ = (ffi::vtable::<ProfileMgrVtbl>(profile_mgr.as_raw()).unregister_profile)(
                profile_mgr.as_raw(),
                &CLSID,
                LANGUAGE,
                &PROFILE,
                0,
            );
        }
        drop(class);
        let _ = registry::RegDeleteTreeW(HKLM, wide(CLASS_KEY).as_ptr());
    }
    result
}

pub unsafe fn register() -> HResult {
    match project_files::dll_path().and_then(|path| {
        if cfg!(target_arch = "x86") {
            register_class_only(&path)
        } else {
            register_inner(&path)
        }
    }) {
        Ok(()) => ffi::S_OK,
        Err(hr) => hr,
    }
}

pub unsafe fn unregister() -> HResult {
    let result = if cfg!(target_arch = "x86") {
        let server = wide(&format!(r"{CLASS_KEY}\InprocServer32"));
        let status = registry::RegDeleteKeyExW(HKLM, server.as_ptr(), 0x200, 0);
        if status == 0 || status == 2 {
            Ok(())
        } else {
            Err(win32_error(status))
        }
    } else {
        unregister_inner()
    };
    match result {
        Ok(()) => ffi::S_OK,
        Err(hr) => hr,
    }
}

/// 已安裝舊版時，只更新指定支援類別，不重建既有設定檔。
pub unsafe fn install_capability(install: bool, category: *const ffi::Guid) -> HResult {
    let result = (|| {
        let _apartment = ComApartment::enter()?;
        let category_mgr = categories()?;
        let table = ffi::vtable::<CategoryMgrVtbl>(category_mgr.as_raw());
        let hr = if install {
            (table.register_category)(category_mgr.as_raw(), &CLSID, category, &CLSID)
        } else {
            (table.unregister_category)(category_mgr.as_raw(), &CLSID, category, &CLSID)
        };
        if hr < 0 {
            Err(hr)
        } else {
            Ok(())
        }
    })();
    match result {
        Ok(()) => ffi::S_OK,
        Err(hr) => hr,
    }
}

unsafe fn unregister_inner() -> Result<(), HResult> {
    let _apartment = ComApartment::enter()?;
    let profile_mgr = profiles()?;
    let category_mgr = categories()?;
    let hr = (ffi::vtable::<ProfileMgrVtbl>(profile_mgr.as_raw()).unregister_profile)(
        profile_mgr.as_raw(),
        &CLSID,
        LANGUAGE,
        &PROFILE,
        0,
    );
    if hr < 0 {
        return Err(hr);
    }
    for category in profile_categories().into_iter().rev() {
        let hr = (ffi::vtable::<CategoryMgrVtbl>(category_mgr.as_raw()).unregister_category)(
            category_mgr.as_raw(),
            &CLSID,
            category,
            &CLSID,
        );
        if hr < 0 {
            return Err(hr);
        }
    }
    let status = registry::RegDeleteTreeW(HKLM, wide(CLASS_KEY).as_ptr());
    if status == 0 || status == 2 {
        Ok(())
    } else {
        Err(win32_error(status))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_attribute_category_matches_update_script() {
        let expected = ffi::Guid::new(
            0x046b8c80,
            0x1647,
            0x40f7,
            [0x9b, 0x21, 0xb9, 0x3b, 0x81, 0xaa, 0xbc, 0x1b],
        );
        assert_eq!(
            unsafe { ffi::GUID_TFCAT_DISPLAYATTRIBUTEPROVIDER },
            expected
        );
    }

    #[test]
    fn uielement_category_matches_update_script() {
        let expected = ffi::Guid::new(
            0x49d2f9cf,
            0x1f5e,
            0x11d7,
            [0xa6, 0xd3, 0, 6, 0x5b, 0x84, 0x43, 0x5c],
        );
        assert_eq!(unsafe { ffi::GUID_TFCAT_TIPCAP_UIELEMENTENABLED }, expected);
    }

    #[test]
    fn immersive_category_matches_update_script() {
        let expected = ffi::Guid::new(
            0x13a016df,
            0x560b,
            0x46cd,
            [0x94, 0x7a, 0x4c, 0x3a, 0xf1, 0xe0, 0xe3, 0x5d],
        );
        assert_eq!(unsafe { ffi::GUID_TFCAT_TIPCAP_IMMERSIVESUPPORT }, expected);
    }
}
