//! 向 TSF 宿主提供組字中的實線底線。
use std::ptr;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use super::ffi::{
    self, DisplayAttribute, DisplayAttributeInfoVtbl, EnumDisplayAttributeInfoVtbl, Guid, HResult,
    Raw, UnknownVtbl,
};
use super::OBJECTS;

pub(crate) const INPUT_ATTRIBUTE: Guid = Guid::new(
    0x240e3ecc,
    0xa260,
    0x41cf,
    [0x98, 0x5a, 0xe1, 0x72, 0x52, 0x57, 0x28, 0x03],
);

#[repr(C)]
struct Info {
    vtbl: *const DisplayAttributeInfoVtbl,
    refs: AtomicU32,
}

#[repr(C)]
struct Enumerator {
    vtbl: *const EnumDisplayAttributeInfoVtbl,
    refs: AtomicU32,
    used: AtomicBool,
}

static INFO_VTBL: DisplayAttributeInfoVtbl = DisplayAttributeInfoVtbl {
    base: UnknownVtbl {
        query: info_query,
        add_ref: info_add_ref,
        release: info_release,
    },
    get_guid: info_guid,
    get_description: info_description,
    get_attribute: info_attribute,
    set_attribute: info_set,
    reset: info_reset,
};

static ENUM_VTBL: EnumDisplayAttributeInfoVtbl = EnumDisplayAttributeInfoVtbl {
    base: UnknownVtbl {
        query: enum_query,
        add_ref: enum_add_ref,
        release: enum_release,
    },
    clone_enum,
    next: enum_next,
    reset: enum_reset,
    skip: enum_skip,
};

unsafe fn new_info() -> Raw {
    let info = Box::into_raw(Box::new(Info {
        vtbl: &INFO_VTBL,
        refs: AtomicU32::new(1),
    }));
    OBJECTS.fetch_add(1, Ordering::SeqCst);
    info.cast()
}

unsafe fn new_enum(used: bool) -> Raw {
    let value = Box::into_raw(Box::new(Enumerator {
        vtbl: &ENUM_VTBL,
        refs: AtomicU32::new(1),
        used: AtomicBool::new(used),
    }));
    OBJECTS.fetch_add(1, Ordering::SeqCst);
    value.cast()
}

pub(crate) unsafe fn enumerate(out: *mut Raw) -> HResult {
    if out.is_null() {
        return ffi::E_POINTER;
    }
    *out = new_enum(false);
    ffi::S_OK
}

pub(crate) unsafe fn get_info(guid: *const Guid, out: *mut Raw) -> HResult {
    if guid.is_null() || out.is_null() {
        return ffi::E_POINTER;
    }
    *out = ptr::null_mut();
    if *guid != INPUT_ATTRIBUTE {
        return ffi::E_INVALIDARG;
    }
    *out = new_info();
    ffi::S_OK
}

unsafe extern "system" fn info_query(this: Raw, iid: *const Guid, out: *mut Raw) -> HResult {
    if out.is_null() {
        return ffi::E_POINTER;
    }
    *out = ptr::null_mut();
    if iid.is_null() {
        return ffi::E_POINTER;
    }
    if *iid != ffi::IID_IUnknown && *iid != ffi::IID_ITfDisplayAttributeInfo {
        return ffi::E_NOINTERFACE;
    }
    *out = this;
    info_add_ref(this);
    ffi::S_OK
}
unsafe extern "system" fn info_add_ref(this: Raw) -> u32 {
    (*(this as *mut Info)).refs.fetch_add(1, Ordering::Relaxed) + 1
}
unsafe extern "system" fn info_release(this: Raw) -> u32 {
    let object = this as *mut Info;
    let refs = (*object).refs.fetch_sub(1, Ordering::Release) - 1;
    if refs == 0 {
        std::sync::atomic::fence(Ordering::Acquire);
        drop(Box::from_raw(object));
        OBJECTS.fetch_sub(1, Ordering::SeqCst);
    }
    refs
}
unsafe extern "system" fn info_guid(_this: Raw, out: *mut Guid) -> HResult {
    if out.is_null() {
        return ffi::E_POINTER;
    }
    *out = INPUT_ATTRIBUTE;
    ffi::S_OK
}
unsafe extern "system" fn info_description(_this: Raw, out: *mut Raw) -> HResult {
    ffi::write_bstr("倉頡組字", out)
}
unsafe extern "system" fn info_attribute(_this: Raw, out: *mut DisplayAttribute) -> HResult {
    if out.is_null() {
        return ffi::E_POINTER;
    }
    *out = DisplayAttribute {
        line_style: 1,
        ..Default::default()
    }; // TF_LS_SOLID、TF_ATTR_INPUT
    ffi::S_OK
}
unsafe extern "system" fn info_set(_this: Raw, value: *const DisplayAttribute) -> HResult {
    if value.is_null() {
        ffi::E_POINTER
    } else {
        ffi::E_NOTIMPL
    }
}
unsafe extern "system" fn info_reset(_this: Raw) -> HResult {
    ffi::S_OK
}

unsafe extern "system" fn enum_query(this: Raw, iid: *const Guid, out: *mut Raw) -> HResult {
    if out.is_null() {
        return ffi::E_POINTER;
    }
    *out = ptr::null_mut();
    if iid.is_null() {
        return ffi::E_POINTER;
    }
    if *iid != ffi::IID_IUnknown && *iid != ffi::IID_IEnumTfDisplayAttributeInfo {
        return ffi::E_NOINTERFACE;
    }
    *out = this;
    enum_add_ref(this);
    ffi::S_OK
}
unsafe extern "system" fn enum_add_ref(this: Raw) -> u32 {
    (*(this as *mut Enumerator))
        .refs
        .fetch_add(1, Ordering::Relaxed)
        + 1
}
unsafe extern "system" fn enum_release(this: Raw) -> u32 {
    let object = this as *mut Enumerator;
    let refs = (*object).refs.fetch_sub(1, Ordering::Release) - 1;
    if refs == 0 {
        std::sync::atomic::fence(Ordering::Acquire);
        drop(Box::from_raw(object));
        OBJECTS.fetch_sub(1, Ordering::SeqCst);
    }
    refs
}
unsafe extern "system" fn clone_enum(this: Raw, out: *mut Raw) -> HResult {
    if out.is_null() {
        return ffi::E_POINTER;
    }
    *out = new_enum((*(this as *mut Enumerator)).used.load(Ordering::Relaxed));
    ffi::S_OK
}
unsafe extern "system" fn enum_next(
    this: Raw,
    count: u32,
    out: *mut Raw,
    fetched: *mut u32,
) -> HResult {
    if count > 0 && out.is_null() || count != 1 && fetched.is_null() {
        return ffi::E_POINTER;
    }
    if !fetched.is_null() {
        *fetched = 0;
    }
    if count == 0 {
        return ffi::S_OK;
    }
    *out = ptr::null_mut();
    if (*(this as *mut Enumerator))
        .used
        .swap(true, Ordering::Relaxed)
    {
        return ffi::S_FALSE;
    }
    *out = new_info();
    if !fetched.is_null() {
        *fetched = 1;
    }
    if count == 1 {
        ffi::S_OK
    } else {
        ffi::S_FALSE
    }
}
unsafe extern "system" fn enum_reset(this: Raw) -> HResult {
    (*(this as *mut Enumerator))
        .used
        .store(false, Ordering::Relaxed);
    ffi::S_OK
}
unsafe extern "system" fn enum_skip(this: Raw, count: u32) -> HResult {
    if count == 0 {
        return ffi::S_OK;
    }
    let was_used = (*(this as *mut Enumerator))
        .used
        .swap(true, Ordering::Relaxed);
    if !was_used && count == 1 {
        ffi::S_OK
    } else {
        ffi::S_FALSE
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tsf::ffi::DisplayAttributeProviderVtbl;

    #[test]
    fn provider_enumerates_a_solid_underline() {
        unsafe {
            let mut provider = ptr::null_mut();
            assert_eq!(
                crate::tsf::service::create(&ffi::IID_ITfDisplayAttributeProvider, &mut provider),
                ffi::S_OK
            );
            let provider_vtbl = ffi::vtable::<DisplayAttributeProviderVtbl>(provider);
            let mut info = ptr::null_mut();
            assert_eq!(
                (provider_vtbl.get_info)(provider, &INPUT_ATTRIBUTE, &mut info),
                ffi::S_OK
            );
            let info_vtbl = ffi::vtable::<DisplayAttributeInfoVtbl>(info);
            let mut guid = Guid::new(0, 0, 0, [0; 8]);
            assert_eq!((info_vtbl.get_guid)(info, &mut guid), ffi::S_OK);
            assert_eq!(guid, INPUT_ATTRIBUTE);
            let mut attribute = DisplayAttribute::default();
            assert_eq!((info_vtbl.get_attribute)(info, &mut attribute), ffi::S_OK);
            assert_eq!(attribute.line_style, 1);
            assert_eq!(attribute.bold_line, 0);
            assert_eq!(attribute.attribute, 0);
            ffi::release(info);

            let mut enumeration = ptr::null_mut();
            assert_eq!(
                (provider_vtbl.enumerate)(provider, &mut enumeration),
                ffi::S_OK
            );
            let table = ffi::vtable::<EnumDisplayAttributeInfoVtbl>(enumeration);
            let mut fetched = 0;
            let mut item = ptr::null_mut();
            assert_eq!(
                (table.next)(enumeration, 1, &mut item, &mut fetched),
                ffi::S_OK
            );
            assert_eq!(fetched, 1);
            ffi::release(item);
            assert_eq!(
                (table.next)(enumeration, 1, &mut item, &mut fetched),
                ffi::S_FALSE
            );
            assert_eq!(fetched, 0);
            assert_eq!((table.reset)(enumeration), ffi::S_OK);
            assert_eq!((table.skip)(enumeration, 1), ffi::S_OK);
            let mut clone = ptr::null_mut();
            assert_eq!((table.clone_enum)(enumeration, &mut clone), ffi::S_OK);
            assert_eq!(
                (table.next)(clone, 1, &mut item, &mut fetched),
                ffi::S_FALSE
            );
            ffi::release(clone);
            ffi::release(enumeration);
            ffi::release(provider);
        }
        assert_eq!(
            std::mem::size_of::<ffi::Variant>(),
            if cfg!(target_pointer_width = "64") {
                24
            } else {
                16
            }
        );
    }
}
