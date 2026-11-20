//! 透過標準 TSF 閱讀提示公開狀態文字，亦允許宿主選擇自行繪製。
use std::ptr;
use std::sync::atomic::{AtomicU32, Ordering};

use super::Registration;
use crate::tsf::ffi::{
    self, ComPtr, Guid, HResult, Raw, ReadingUiVtbl, UiElementVtbl, UnknownVtbl,
};
use crate::tsf::OBJECTS;

const GUID: Guid = Guid::new(
    0x93b8a3fa,
    0xe6c1,
    0x4872,
    [0x9b, 0x80, 0xa2, 0x1c, 0x66, 0x0f, 0x74, 0x20],
);

#[repr(C)]
struct Element {
    vtbl: *const ReadingUiVtbl,
    refs: AtomicU32,
    owner: Raw,
    callback: unsafe fn(Raw, bool),
    label: &'static str,
    context: Option<ComPtr>,
    shown: bool,
}

static VTABLE: ReadingUiVtbl = ReadingUiVtbl {
    base: UiElementVtbl {
        base: UnknownVtbl {
            query,
            add_ref,
            release,
        },
        description,
        guid,
        show,
        is_shown,
    },
    flags,
    context: get_context,
    string: description,
    max_length,
    error_index,
    vertical,
};

pub(crate) struct ToastUi {
    registration: Registration,
    element: Raw,
}

impl ToastUi {
    pub(crate) fn new() -> Self {
        Self {
            registration: Registration::default(),
            element: ptr::null_mut(),
        }
    }

    pub(crate) unsafe fn attach(
        &mut self,
        manager: Raw,
        owner: Raw,
        callback: unsafe fn(Raw, bool),
        require_manager: bool,
    ) {
        self.registration.attach(manager, require_manager);
        self.element = Box::into_raw(Box::new(Element {
            vtbl: &VTABLE,
            refs: AtomicU32::new(1),
            owner,
            callback,
            label: "英",
            context: None,
            shown: false,
        }))
        .cast();
        OBJECTS.fetch_add(1, Ordering::SeqCst);
    }

    pub(crate) unsafe fn begin(this: *mut Self, label: &'static str, context: Raw) -> bool {
        Self::end(this);
        if (*this).element.is_null() {
            return (*this).registration.fallback_show();
        }
        let element = (*this).element as *mut Element;
        (*element).label = label;
        (*element).context = ((*this).registration.has_manager() && !context.is_null())
            .then(|| ComPtr::retain(context));
        let registration = ptr::addr_of_mut!((*this).registration);
        (*element).shown = Registration::begin(registration, (*this).element);
        if !(*element).shown {
            Registration::update(registration);
        }
        (*element).shown
    }

    pub(crate) unsafe fn end(this: *mut Self) {
        Registration::end(ptr::addr_of_mut!((*this).registration));
        if !(*this).element.is_null() {
            let element = (*this).element as *mut Element;
            (*element).shown = false;
            drop((*element).context.take());
        }
    }

    pub(crate) unsafe fn detach(this: *mut Self) {
        if !(*this).element.is_null() {
            (*((*this).element as *mut Element)).owner = ptr::null_mut();
        }
        Self::end(this);
        let element = std::mem::replace(&mut (*this).element, ptr::null_mut());
        if !element.is_null() {
            ffi::release(element);
        }
        drop((*this).registration.manager.take());
    }
}

impl Drop for ToastUi {
    fn drop(&mut self) {
        unsafe { Self::detach(self) }
    }
}

unsafe extern "system" fn query(this: Raw, iid: *const Guid, out: *mut Raw) -> HResult {
    if out.is_null() {
        return ffi::E_POINTER;
    }
    *out = ptr::null_mut();
    if iid.is_null() {
        return ffi::E_POINTER;
    }
    if *iid != ffi::IID_IUnknown
        && *iid != ffi::IID_ITfUIElement
        && *iid != ffi::IID_ITfReadingInformationUIElement
    {
        return ffi::E_NOINTERFACE;
    }
    *out = this;
    add_ref(this);
    ffi::S_OK
}
unsafe extern "system" fn add_ref(this: Raw) -> u32 {
    (*(this as *mut Element))
        .refs
        .fetch_add(1, Ordering::Relaxed)
        + 1
}
unsafe extern "system" fn release(this: Raw) -> u32 {
    let element = this as *mut Element;
    let refs = (*element).refs.fetch_sub(1, Ordering::Release) - 1;
    if refs == 0 {
        std::sync::atomic::fence(Ordering::Acquire);
        drop(Box::from_raw(element));
        OBJECTS.fetch_sub(1, Ordering::SeqCst);
    }
    refs
}
unsafe extern "system" fn description(this: Raw, out: *mut Raw) -> HResult {
    ffi::write_bstr((*(this as *mut Element)).label, out)
}
unsafe extern "system" fn guid(_: Raw, out: *mut Guid) -> HResult {
    if out.is_null() {
        return ffi::E_POINTER;
    }
    *out = GUID;
    ffi::S_OK
}
unsafe extern "system" fn show(this: Raw, value: i32) -> HResult {
    let element = this as *mut Element;
    (*element).shown = value != 0;
    let owner = (*element).owner;
    if !owner.is_null() {
        ((*element).callback)(owner, value != 0);
    }
    ffi::S_OK
}
unsafe extern "system" fn is_shown(this: Raw, out: *mut i32) -> HResult {
    if out.is_null() {
        return ffi::E_POINTER;
    }
    *out = (*(this as *mut Element)).shown as i32;
    ffi::S_OK
}

unsafe extern "system" fn flags(_: Raw, out: *mut u32) -> HResult {
    if out.is_null() {
        return ffi::E_POINTER;
    }
    *out = 0x1f; // TF_RIUIE_*：新的提示可更新所有欄位。
    ffi::S_OK
}

unsafe extern "system" fn get_context(this: Raw, out: *mut Raw) -> HResult {
    if out.is_null() {
        return ffi::E_POINTER;
    }
    *out = ptr::null_mut();
    let Some(context) = (*(this as *const Element)).context.as_ref() else {
        return ffi::S_FALSE;
    };
    *out = context.as_raw();
    ffi::add_ref(*out);
    ffi::S_OK
}

unsafe extern "system" fn max_length(this: Raw, out: *mut u32) -> HResult {
    if out.is_null() {
        return ffi::E_POINTER;
    }
    *out = (*(this as *const Element)).label.encode_utf16().count() as u32;
    ffi::S_OK
}

unsafe extern "system" fn error_index(_: Raw, out: *mut u32) -> HResult {
    if out.is_null() {
        return ffi::E_POINTER;
    }
    *out = u32::MAX;
    ffi::S_OK
}

unsafe extern "system" fn vertical(_: Raw, out: *mut i32) -> HResult {
    if out.is_null() {
        return ffi::E_POINTER;
    }
    *out = 0;
    ffi::S_OK
}
