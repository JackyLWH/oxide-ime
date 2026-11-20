//! 透過 TSF UIElement 與應用程式協商候選框顯示。
use std::ptr;
use std::sync::atomic::{AtomicU32, Ordering};

use super::Registration;
use crate::engine::Session;
use crate::tsf::ffi::{
    self, CandidateUiVtbl, ComPtr, ContextFullVtbl, Guid, HResult, Raw, UnknownVtbl,
};
use crate::tsf::OBJECTS;

const ELEMENT_GUID: Guid = Guid::new(
    0x4b8697e4,
    0x1a42,
    0x4f55,
    [0x84, 0xc7, 0xad, 0x9f, 0x33, 0xef, 0x1a, 0xa2],
);

pub(crate) struct Callbacks {
    pub owner: Raw,
    pub set_selection: unsafe fn(Raw, usize) -> bool,
    pub finalize: unsafe fn(Raw) -> bool,
    pub abort: unsafe fn(Raw) -> bool,
    pub show: unsafe fn(Raw, bool),
}

#[repr(C)]
struct Element {
    vtbl: *const CandidateUiVtbl,
    refs: AtomicU32,
    callbacks: Callbacks,
    snapshot: Option<Session>,
    context: Option<ComPtr>,
    shown: bool,
    pages: Vec<u32>,
}

impl Element {
    fn context(&self) -> Raw {
        self.context
            .as_ref()
            .map_or(ptr::null_mut(), |context| context.as_raw())
    }
}

static VTABLE: CandidateUiVtbl = CandidateUiVtbl {
    base: UnknownVtbl {
        query,
        add_ref,
        release,
    },
    description,
    guid,
    show,
    is_shown,
    flags,
    document,
    count,
    selection,
    string,
    page_index,
    set_page_index,
    current_page,
    set_selection,
    finalize,
    abort,
};

pub(crate) struct CandidateUi {
    registration: Registration,
    element: Raw,
}

impl CandidateUi {
    pub(crate) fn new() -> Self {
        Self {
            registration: Registration::default(),
            element: ptr::null_mut(),
        }
    }

    pub(crate) unsafe fn attach(
        &mut self,
        manager: Raw,
        callbacks: Callbacks,
        require_manager: bool,
    ) {
        self.registration.attach(manager, require_manager);
        let element = Box::into_raw(Box::new(Element {
            vtbl: &VTABLE,
            refs: AtomicU32::new(1),
            callbacks,
            snapshot: None,
            context: None,
            shown: false,
            pages: Vec::new(),
        }));
        OBJECTS.fetch_add(1, Ordering::SeqCst);
        self.element = element.cast();
    }

    pub(crate) fn has_manager(&self) -> bool {
        self.registration.has_manager()
    }

    pub(crate) unsafe fn show_allowed(this: *const Self) -> bool {
        if (*this).element.is_null() {
            (*this).registration.fallback_show()
        } else {
            (*((*this).element as *const Element)).shown
        }
    }

    pub(crate) unsafe fn context(this: *const Self) -> Raw {
        if (*this).element.is_null() {
            ptr::null_mut()
        } else {
            (*((*this).element as *const Element)).context()
        }
    }

    pub(crate) unsafe fn replace_snapshot(this: *mut Self, snapshot: Session) {
        if !(*this).element.is_null() {
            (*((*this).element as *mut Element)).snapshot = Some(snapshot);
        }
    }

    /// 傳入原始指標，避免 TSF 同步回呼時保留跨 COM 呼叫的 Rust 可變參考。
    pub(crate) unsafe fn update(this: *mut Self, snapshot: Session, context: Raw) -> bool {
        if (*this).element.is_null() {
            return (*this).registration.fallback_show();
        }
        let element = (*this).element as *mut Element;
        let count = snapshot.ui_count();
        if count == 0 {
            Self::end(this);
            return false;
        }
        let unchanged = (*element).snapshot.as_ref().is_some_and(|previous| {
            previous.ui_count() == count
                && previous.menu_open == snapshot.menu_open
                && previous.page_size() == snapshot.page_size()
        });
        if !unchanged || (*element).pages.is_empty() {
            let step = if snapshot.menu_open {
                count
            } else {
                snapshot.page_size()
            };
            (*element).pages.clear();
            (*element)
                .pages
                .extend((0..count).step_by(step).map(|n| n as u32));
        }
        (*element).snapshot = Some(snapshot);
        if (*element).context() != context {
            let context = (!context.is_null()).then(|| ComPtr::retain(context));
            drop(std::mem::replace(&mut (*element).context, context));
        }
        let registration = ptr::addr_of_mut!((*this).registration);
        if (*registration).id.is_some() {
            Registration::update(registration);
        } else {
            (*element).shown = Registration::begin(registration, (*this).element);
            if !(*element).shown {
                // 由宿主繪製時，首次 Begin 後也要通知宿主讀取候選內容。
                Registration::update(registration);
            }
        }
        (*element).shown
    }

    pub(crate) unsafe fn end(this: *mut Self) {
        Registration::end(ptr::addr_of_mut!((*this).registration));
        if !(*this).element.is_null() {
            let element = (*this).element as *mut Element;
            (*element).snapshot = None;
            (*element).shown = false;
            (*element).pages.clear();
            drop((*element).context.take());
        }
    }

    pub(crate) unsafe fn detach(this: *mut Self) {
        if !(*this).element.is_null() {
            (*((*this).element as *mut Element)).callbacks.owner = ptr::null_mut();
        }
        Self::end(this);
        let element = std::mem::replace(&mut (*this).element, ptr::null_mut());
        if !element.is_null() {
            ffi::release(element);
        }
        drop((*this).registration.manager.take());
    }
}

impl Drop for CandidateUi {
    fn drop(&mut self) {
        unsafe { Self::detach(self) }
    }
}

unsafe extern "system" fn query(this: Raw, iid: *const Guid, out: *mut Raw) -> HResult {
    if iid.is_null() || out.is_null() {
        return ffi::E_POINTER;
    }
    *out = ptr::null_mut();
    if *iid != ffi::IID_IUnknown
        && *iid != ffi::IID_ITfUIElement
        && *iid != ffi::IID_ITfCandidateListUIElement
        && *iid != ffi::IID_ITfCandidateListUIElementBehavior
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

unsafe extern "system" fn description(_: Raw, out: *mut Raw) -> HResult {
    ffi::write_bstr("Oxide IME 候選", out)
}
unsafe extern "system" fn guid(_: Raw, out: *mut Guid) -> HResult {
    if out.is_null() {
        return ffi::E_POINTER;
    }
    *out = ELEMENT_GUID;
    ffi::S_OK
}
unsafe extern "system" fn show(this: Raw, value: i32) -> HResult {
    let element = this as *mut Element;
    (*element).shown = value != 0;
    let owner = (*element).callbacks.owner;
    if !owner.is_null() {
        ((*element).callbacks.show)(owner, value != 0);
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
    *out = 0x3f;
    ffi::S_OK
}
unsafe extern "system" fn document(this: Raw, out: *mut Raw) -> HResult {
    if out.is_null() {
        return ffi::E_POINTER;
    }
    *out = ptr::null_mut();
    let context = (*(this as *mut Element)).context();
    if context.is_null() {
        return ffi::E_FAIL;
    }
    (ffi::vtable::<ContextFullVtbl>(context).get_document_mgr)(context, out)
}
unsafe extern "system" fn count(this: Raw, out: *mut u32) -> HResult {
    if out.is_null() {
        return ffi::E_POINTER;
    }
    *out = (*(this as *mut Element))
        .snapshot
        .as_ref()
        .map_or(0, |s| s.ui_count() as u32);
    ffi::S_OK
}
unsafe extern "system" fn selection(this: Raw, out: *mut u32) -> HResult {
    if out.is_null() {
        return ffi::E_POINTER;
    }
    let selected = (*(this as *mut Element))
        .snapshot
        .as_ref()
        .and_then(Session::ui_selection);
    if let Some(selected) = selected {
        *out = selected as u32;
        ffi::S_OK
    } else {
        ffi::S_FALSE
    }
}
unsafe extern "system" fn string(this: Raw, index: u32, out: *mut Raw) -> HResult {
    if out.is_null() {
        return ffi::E_POINTER;
    }
    *out = ptr::null_mut();
    match (*(this as *mut Element))
        .snapshot
        .as_ref()
        .and_then(|s| s.ui_text(index as usize))
    {
        Some(value) => ffi::write_bstr(&value, out),
        None => ffi::E_INVALIDARG,
    }
}
unsafe extern "system" fn page_index(
    this: Raw,
    out: *mut u32,
    size: u32,
    count: *mut u32,
) -> HResult {
    if count.is_null() || (size > 0 && out.is_null()) {
        return ffi::E_POINTER;
    }
    let pages = &(*(this as *mut Element)).pages;
    *count = pages.len() as u32;
    for (i, page) in pages.iter().take(size as usize).enumerate() {
        *out.add(i) = *page;
    }
    ffi::S_OK
}
unsafe extern "system" fn set_page_index(this: Raw, pages: *const u32, count: u32) -> HResult {
    if pages.is_null() || count == 0 {
        return ffi::E_INVALIDARG;
    }
    let element = this as *mut Element;
    let total = (*element).snapshot.as_ref().map_or(0, |s| s.ui_count());
    if count as usize > total {
        return ffi::E_INVALIDARG;
    }
    let pages = std::slice::from_raw_parts(pages, count as usize);
    if pages[0] != 0
        || pages.iter().any(|p| *p as usize >= total)
        || pages.windows(2).any(|p| p[0] >= p[1])
    {
        return ffi::E_INVALIDARG;
    }
    pages.clone_into(&mut (*element).pages);
    ffi::S_OK
}
unsafe extern "system" fn current_page(this: Raw, out: *mut u32) -> HResult {
    if out.is_null() {
        return ffi::E_POINTER;
    }
    let element = &*(this as *mut Element);
    let selected = element
        .snapshot
        .as_ref()
        .and_then(Session::ui_selection)
        .unwrap_or(0) as u32;
    *out = element
        .pages
        .partition_point(|&start| start <= selected)
        .saturating_sub(1) as u32;
    ffi::S_OK
}
unsafe extern "system" fn set_selection(this: Raw, index: u32) -> HResult {
    let element = this as *mut Element;
    let owner = (*element).callbacks.owner;
    if owner.is_null() || !((*element).callbacks.set_selection)(owner, index as usize) {
        ffi::E_FAIL
    } else {
        ffi::S_OK
    }
}
unsafe extern "system" fn finalize(this: Raw) -> HResult {
    let element = this as *mut Element;
    let owner = (*element).callbacks.owner;
    if owner.is_null() || !((*element).callbacks.finalize)(owner) {
        ffi::E_FAIL
    } else {
        ffi::S_OK
    }
}
unsafe extern "system" fn abort(this: Raw) -> HResult {
    let element = this as *mut Element;
    let owner = (*element).callbacks.owner;
    if owner.is_null() || !((*element).callbacks.abort)(owner) {
        ffi::E_FAIL
    } else {
        ffi::S_OK
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dictionary::Dictionary;
    use crate::engine::Key;
    use crate::tsf::ffi::UiElementMgrVtbl;
    use std::sync::Arc;

    #[repr(C)]
    struct Manager {
        vtbl: *const UiElementMgrVtbl,
        refs: AtomicU32,
        element: Raw,
        begins: u32,
        updates: u32,
        ends: u32,
    }

    static MANAGER_VTABLE: UiElementMgrVtbl = UiElementMgrVtbl {
        base: UnknownVtbl {
            query: manager_query,
            add_ref: manager_add_ref,
            release: manager_release,
        },
        begin: manager_begin,
        update: manager_update,
        end: manager_end,
    };

    unsafe extern "system" fn manager_query(this: Raw, iid: *const Guid, out: *mut Raw) -> HResult {
        if iid.is_null() || out.is_null() {
            return ffi::E_POINTER;
        }
        *out = ptr::null_mut();
        if *iid != ffi::IID_IUnknown && *iid != ffi::IID_ITfUIElementMgr {
            return ffi::E_NOINTERFACE;
        }
        *out = this;
        manager_add_ref(this);
        ffi::S_OK
    }
    unsafe extern "system" fn manager_add_ref(this: Raw) -> u32 {
        (*(this as *mut Manager))
            .refs
            .fetch_add(1, Ordering::SeqCst)
            + 1
    }
    unsafe extern "system" fn manager_release(this: Raw) -> u32 {
        (*(this as *mut Manager))
            .refs
            .fetch_sub(1, Ordering::SeqCst)
            - 1
    }
    unsafe extern "system" fn manager_begin(
        this: Raw,
        element: Raw,
        show: *mut i32,
        id: *mut u32,
    ) -> HResult {
        let manager = &mut *(this as *mut Manager);
        manager.begins += 1;
        manager.element = element;
        ffi::add_ref(element);
        *show = 0;
        *id = 17;
        ffi::S_OK
    }
    unsafe extern "system" fn manager_update(this: Raw, id: u32) -> HResult {
        assert_eq!(id, 17);
        (*(this as *mut Manager)).updates += 1;
        ffi::S_OK
    }
    unsafe extern "system" fn manager_end(this: Raw, id: u32) -> HResult {
        assert_eq!(id, 17);
        let manager = &mut *(this as *mut Manager);
        manager.ends += 1;
        let element = std::mem::replace(&mut manager.element, ptr::null_mut());
        ffi::release(element);
        ffi::S_OK
    }

    #[derive(Default)]
    struct Calls {
        selection: usize,
        finalized: usize,
        aborted: usize,
        shown: bool,
    }
    unsafe fn selected(owner: Raw, index: usize) -> bool {
        (*(owner as *mut Calls)).selection = index;
        true
    }
    unsafe fn finalized(owner: Raw) -> bool {
        (*(owner as *mut Calls)).finalized += 1;
        true
    }
    unsafe fn aborted(owner: Raw) -> bool {
        (*(owner as *mut Calls)).aborted += 1;
        true
    }
    unsafe fn shown(owner: Raw, value: bool) {
        (*(owner as *mut Calls)).shown = value;
    }

    #[link(name = "oleaut32")]
    extern "system" {
        fn SysStringLen(value: Raw) -> u32;
        fn SysFreeString(value: Raw);
    }

    #[test]
    fn uiless_host_can_read_and_control_candidates_without_forcing_a_window() {
        let root = crate::tests::fixture_dir("uielement-test");
        std::fs::write(
            root.join("cangjie3.dict.yaml"),
            "---\n...\n日\ta\n曰\ta\n昌\ta\n",
        )
        .unwrap();
        std::fs::write(root.join("custom.dict.yaml"), "---\n...\n").unwrap();
        let mut session = Session::new(Arc::new(
            Dictionary::load(&root.join("cangjie3.dict.yaml"), None).unwrap(),
        ));
        session.handle(Key::ToggleMode);
        session.handle(Key::Letter('a'));
        let mut calls = Calls::default();
        let mut manager = Manager {
            vtbl: &MANAGER_VTABLE,
            refs: AtomicU32::new(1),
            element: ptr::null_mut(),
            begins: 0,
            updates: 0,
            ends: 0,
        };
        let mut ui = CandidateUi::new();
        unsafe {
            ui.attach(
                (&mut manager as *mut Manager).cast(),
                Callbacks {
                    owner: (&mut calls as *mut Calls).cast(),
                    set_selection: selected,
                    finalize: finalized,
                    abort: aborted,
                    show: shown,
                },
                true,
            );
            assert!(!CandidateUi::update(
                &mut ui,
                session.clone(),
                ptr::null_mut()
            ));
            assert_eq!(manager.begins, 1);
            assert_eq!(manager.updates, 1);
            let element = manager.element;
            let table = ffi::vtable::<CandidateUiVtbl>(element);
            let mut value = 0;
            assert_eq!((table.count)(element, &mut value), ffi::S_OK);
            assert_eq!(value, 3);
            assert_eq!((table.selection)(element, &mut value), ffi::S_OK);
            assert_eq!(value, 0);
            let mut text = ptr::null_mut();
            assert_eq!((table.string)(element, 2, &mut text), ffi::S_OK);
            let slice = std::slice::from_raw_parts(text.cast::<u16>(), SysStringLen(text) as usize);
            assert_eq!(String::from_utf16_lossy(slice), "昌");
            SysFreeString(text);
            let mut pages = [0u32; 2];
            assert_eq!(
                (table.page_index)(element, pages.as_mut_ptr(), 2, &mut value),
                ffi::S_OK
            );
            assert_eq!((pages, value), ([0, 2], 2));
            assert_eq!((table.current_page)(element, &mut value), ffi::S_OK);
            assert_eq!(value, 0);
            assert_eq!((table.set_selection)(element, 2), ffi::S_OK);
            assert_eq!(calls.selection, 2);
            assert_eq!((table.finalize)(element), ffi::S_OK);
            assert_eq!((table.abort)(element), ffi::S_OK);
            assert_eq!((calls.finalized, calls.aborted), (1, 1));
            assert_eq!((table.show)(element, 1), ffi::S_OK);
            assert!(calls.shown && CandidateUi::show_allowed(&ui));
            session.handle(Key::Next);
            session.handle(Key::Next);
            assert!(CandidateUi::update(&mut ui, session, ptr::null_mut()));
            assert_eq!(manager.updates, 2);
            assert_eq!((table.current_page)(element, &mut value), ffi::S_OK);
            assert_eq!(value, 1);
            assert_eq!(
                (table.set_page_index)(element, pages.as_ptr(), 1),
                ffi::S_OK
            );
            assert_eq!((table.current_page)(element, &mut value), ffi::S_OK);
            assert_eq!(value, 0);
            CandidateUi::end(&mut ui);
            assert_eq!(manager.ends, 1);
            CandidateUi::detach(&mut ui);
            assert_eq!(manager.refs.load(Ordering::SeqCst), 1);
        }
        crate::tests::remove_fixture(&root);
    }
}
