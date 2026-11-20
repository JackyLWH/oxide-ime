//! COM 類別工廠及 DLL 的物件生命週期。
use std::ffi::c_void;
use std::ptr;
use std::sync::atomic::{AtomicU32, Ordering};

use super::ffi::{self, ClassFactoryVtbl, Guid, HResult, Raw, UnknownVtbl};
use super::{service, OBJECTS};

#[repr(C)]
struct Factory {
    vtbl: *const ClassFactoryVtbl,
    refs: AtomicU32,
}

static FACTORY_VTBL: ClassFactoryVtbl = ClassFactoryVtbl {
    base: UnknownVtbl {
        query,
        add_ref,
        release,
    },
    create_instance,
    lock_server,
};

pub unsafe fn create_factory(iid: *const Guid, out: *mut Raw) -> HResult {
    let factory = Box::into_raw(Box::new(Factory {
        vtbl: &FACTORY_VTBL,
        refs: AtomicU32::new(1),
    }));
    OBJECTS.fetch_add(1, Ordering::SeqCst);
    let hr = query(factory.cast(), iid, out);
    release(factory.cast());
    hr
}

unsafe extern "system" fn query(this: Raw, iid: *const Guid, out: *mut Raw) -> HResult {
    if out.is_null() {
        return ffi::E_POINTER;
    }
    *out = ptr::null_mut();
    if iid.is_null() {
        return ffi::E_POINTER;
    }
    if *iid == ffi::IID_IUnknown || *iid == ffi::IID_IClassFactory {
        *out = this;
        add_ref(this);
        ffi::S_OK
    } else {
        ffi::E_NOINTERFACE
    }
}

unsafe extern "system" fn add_ref(this: Raw) -> u32 {
    (*(this as *mut Factory))
        .refs
        .fetch_add(1, Ordering::Relaxed)
        + 1
}

unsafe extern "system" fn release(this: Raw) -> u32 {
    let factory = this as *mut Factory;
    let refs = (*factory).refs.fetch_sub(1, Ordering::Release) - 1;
    if refs == 0 {
        std::sync::atomic::fence(Ordering::Acquire);
        drop(Box::from_raw(factory));
        OBJECTS.fetch_sub(1, Ordering::SeqCst);
    }
    refs
}

unsafe extern "system" fn create_instance(
    _this: Raw,
    outer: Raw,
    iid: *const Guid,
    out: *mut Raw,
) -> HResult {
    if out.is_null() {
        return ffi::E_POINTER;
    }
    *out = ptr::null_mut();
    if iid.is_null() {
        return ffi::E_POINTER;
    }
    if !outer.is_null() {
        return ffi::CLASS_E_NOAGGREGATION;
    }
    service::create(iid, out)
}

unsafe extern "system" fn lock_server(_this: *mut c_void, lock: i32) -> HResult {
    if lock != 0 {
        OBJECTS.fetch_add(1, Ordering::SeqCst);
    } else {
        OBJECTS.fetch_sub(1, Ordering::SeqCst);
    }
    ffi::S_OK
}
