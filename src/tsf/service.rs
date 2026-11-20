//! TSF 文字服務：按鍵、組字生命週期及原生候選窗。
mod edit;
mod input;
mod keys;
mod mouse;
mod presentation;
mod scintilla;

use std::mem::offset_of;
#[cfg(test)]
use std::path::PathBuf;
use std::ptr;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Mutex, OnceLock};

use super::candidate_window::CandidateWindow;
use super::composition::{self, CompositionState};
use super::display_attribute;
use super::ffi::{
    self, CompositionSinkVtbl, DisplayAttributeProviderVtbl, Guid, HResult, KeyEventSinkVtbl,
    KeyStrokeMgrVtbl, Raw, Rect, TextInputProcessorExVtbl, UnknownVtbl,
};
use super::language_bar::LanguageBar;
use super::ui_element::{Callbacks as UiCallbacks, CandidateUi, ToastUi};
#[cfg(test)]
use super::MODULE;
use super::OBJECTS;
use crate::diagnostics::{self, Level};
use crate::dictionary::{Dictionaries, DictionaryStore};
use crate::engine::{Key, Session};
use crate::project_files;
use input::{
    cancel_pending, observe_key_down, perform_key, session_for_keys, synchronize_modes,
    window_event, DeferredInput,
};
use keys::{modifiers, shift_key};
use mouse::remove_mouse_hook;
use presentation::{
    candidate_clicked, mode_icon_clicked, toast_expired, toast_show, ui_abort, ui_finalize,
    ui_set_selection, ui_show,
};

// AppContainer 透過字庫服務操作字典。一般宿主持有本機字庫快取。
static APP_CONTAINER: OnceLock<Result<bool, String>> = OnceLock::new();
static DICTIONARY: Mutex<Option<DictionaryStore>> = Mutex::new(None);

#[cfg(test)]
pub(crate) fn set_test_dictionary(directory: PathBuf) {
    let mut store = DICTIONARY.lock().unwrap();
    assert!(store.is_none());
    *store = Some(DictionaryStore::new(directory));
}

fn dictionary() -> Result<Option<Dictionaries>, HResult> {
    if *APP_CONTAINER
        .get_or_init(crate::broker::is_appcontainer)
        .as_ref()
        .map_err(|_| ffi::E_FAIL)?
    {
        return Ok(None);
    }
    let mut loaded = DICTIONARY
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if loaded.is_none() {
        let root = project_files::dll_root().map_err(|_| ffi::E_FAIL);
        #[cfg(test)]
        let root = if MODULE.load(Ordering::Relaxed).is_null() {
            Ok(PathBuf::from(env!("CARGO_MANIFEST_DIR")))
        } else {
            root
        };
        let root = root?;
        *loaded = Some(DictionaryStore::for_root(&root).map_err(|_| ffi::E_FAIL)?);
    }
    loaded
        .as_mut()
        .ok_or(ffi::E_FAIL)?
        .dictionaries()
        .map(Some)
        .map_err(|_| ffi::E_FAIL)
}

#[repr(C)]
struct TextService {
    processor_vtbl: *const TextInputProcessorExVtbl,
    key_sink_vtbl: *const KeyEventSinkVtbl,
    composition_sink_vtbl: *const CompositionSinkVtbl,
    attribute_provider_vtbl: *const DisplayAttributeProviderVtbl,
    refs: AtomicU32,
    key_mgr: Raw,
    client_id: u32,
    focus_epoch: u32,
    session: Option<Session>,
    composition: CompositionState,
    window: CandidateWindow,
    mode_toast: CandidateWindow,
    language_bar: LanguageBar,
    candidate_ui: CandidateUi,
    toast_ui: ToastUi,
    toast_label: &'static str,
    toast_anchor: Option<Rect>,
    mouse_hook: Raw,
    menu_mouse_hook: Raw,
    deferred: Option<DeferredInput>,
    shift_pending: bool,
    eaten_keys: [bool; 256],
}

static PROCESSOR_VTBL: TextInputProcessorExVtbl = TextInputProcessorExVtbl {
    base: UnknownVtbl {
        query: processor_query,
        add_ref: processor_add_ref,
        release: processor_release,
    },
    activate,
    deactivate,
    activate_ex,
};
static KEY_SINK_VTBL: KeyEventSinkVtbl = KeyEventSinkVtbl {
    base: UnknownVtbl {
        query: key_query,
        add_ref: key_add_ref,
        release: key_release,
    },
    on_set_focus,
    on_test_key_down,
    on_test_key_up,
    on_key_down,
    on_key_up,
    on_preserved_key,
};
static COMPOSITION_SINK_VTBL: CompositionSinkVtbl = CompositionSinkVtbl {
    base: UnknownVtbl {
        query: composition_query,
        add_ref: composition_add_ref,
        release: composition_release,
    },
    terminated: on_composition_terminated,
};
static ATTRIBUTE_PROVIDER_VTBL: DisplayAttributeProviderVtbl = DisplayAttributeProviderVtbl {
    base: UnknownVtbl {
        query: attribute_query,
        add_ref: attribute_add_ref,
        release: attribute_release,
    },
    enumerate: attribute_enumerate,
    get_info: attribute_get_info,
};

pub unsafe fn create(iid: *const Guid, out: *mut Raw) -> HResult {
    let service = Box::into_raw(Box::new(TextService {
        processor_vtbl: &PROCESSOR_VTBL,
        key_sink_vtbl: &KEY_SINK_VTBL,
        composition_sink_vtbl: &COMPOSITION_SINK_VTBL,
        attribute_provider_vtbl: &ATTRIBUTE_PROVIDER_VTBL,
        refs: AtomicU32::new(1),
        key_mgr: ptr::null_mut(),
        client_id: 0,
        focus_epoch: 0,
        session: None,
        composition: CompositionState::new(),
        window: CandidateWindow::new(),
        mode_toast: CandidateWindow::new(),
        language_bar: LanguageBar::new(),
        candidate_ui: CandidateUi::new(),
        toast_ui: ToastUi::new(),
        toast_label: "英",
        toast_anchor: None,
        mouse_hook: ptr::null_mut(),
        menu_mouse_hook: ptr::null_mut(),
        deferred: None,
        shift_pending: false,
        eaten_keys: [false; 256],
    }));
    (*service)
        .window
        .set_click_handler(service.cast(), candidate_clicked);
    (*service).window.set_event_handler(window_event);
    (*service)
        .mode_toast
        .set_timer_handler(service.cast(), toast_expired);
    OBJECTS.fetch_add(1, Ordering::SeqCst);
    let hr = processor_query(service.cast(), iid, out);
    processor_release(service.cast());
    hr
}

unsafe fn key_owner(this: Raw) -> *mut TextService {
    (this as *mut u8)
        .sub(offset_of!(TextService, key_sink_vtbl))
        .cast()
}
unsafe fn composition_owner(this: Raw) -> *mut TextService {
    (this as *mut u8)
        .sub(offset_of!(TextService, composition_sink_vtbl))
        .cast()
}
unsafe fn attribute_owner(this: Raw) -> *mut TextService {
    (this as *mut u8)
        .sub(offset_of!(TextService, attribute_provider_vtbl))
        .cast()
}

unsafe extern "system" fn processor_query(this: Raw, iid: *const Guid, out: *mut Raw) -> HResult {
    if out.is_null() {
        return ffi::E_POINTER;
    }
    *out = ptr::null_mut();
    if iid.is_null() {
        return ffi::E_POINTER;
    }
    let object = this as *mut TextService;
    if *iid == ffi::IID_IUnknown
        || *iid == ffi::IID_ITfTextInputProcessor
        || *iid == ffi::IID_ITfTextInputProcessorEx
    {
        *out = this;
    } else if *iid == ffi::IID_ITfKeyEventSink {
        *out = ptr::addr_of_mut!((*object).key_sink_vtbl).cast();
    } else if *iid == ffi::IID_ITfCompositionSink {
        *out = ptr::addr_of_mut!((*object).composition_sink_vtbl).cast();
    } else if *iid == ffi::IID_ITfDisplayAttributeProvider {
        *out = ptr::addr_of_mut!((*object).attribute_provider_vtbl).cast();
    } else {
        return ffi::E_NOINTERFACE;
    }
    processor_add_ref(this);
    ffi::S_OK
}

unsafe extern "system" fn processor_add_ref(this: Raw) -> u32 {
    (*(this as *mut TextService))
        .refs
        .fetch_add(1, Ordering::Relaxed)
        + 1
}
unsafe extern "system" fn processor_release(this: Raw) -> u32 {
    let object = this as *mut TextService;
    let refs = (*object).refs.fetch_sub(1, Ordering::Release) - 1;
    if refs == 0 {
        std::sync::atomic::fence(Ordering::Acquire);
        remove_mouse_hook(object);
        drop(Box::from_raw(object));
        OBJECTS.fetch_sub(1, Ordering::SeqCst);
    }
    refs
}
unsafe extern "system" fn key_query(this: Raw, iid: *const Guid, out: *mut Raw) -> HResult {
    processor_query(key_owner(this).cast(), iid, out)
}
unsafe extern "system" fn key_add_ref(this: Raw) -> u32 {
    processor_add_ref(key_owner(this).cast())
}
unsafe extern "system" fn key_release(this: Raw) -> u32 {
    processor_release(key_owner(this).cast())
}
unsafe extern "system" fn composition_query(this: Raw, iid: *const Guid, out: *mut Raw) -> HResult {
    processor_query(composition_owner(this).cast(), iid, out)
}
unsafe extern "system" fn composition_add_ref(this: Raw) -> u32 {
    processor_add_ref(composition_owner(this).cast())
}
unsafe extern "system" fn composition_release(this: Raw) -> u32 {
    processor_release(composition_owner(this).cast())
}
unsafe extern "system" fn attribute_query(this: Raw, iid: *const Guid, out: *mut Raw) -> HResult {
    processor_query(attribute_owner(this).cast(), iid, out)
}
unsafe extern "system" fn attribute_add_ref(this: Raw) -> u32 {
    processor_add_ref(attribute_owner(this).cast())
}
unsafe extern "system" fn attribute_release(this: Raw) -> u32 {
    processor_release(attribute_owner(this).cast())
}
unsafe extern "system" fn attribute_enumerate(_this: Raw, out: *mut Raw) -> HResult {
    display_attribute::enumerate(out)
}
unsafe extern "system" fn attribute_get_info(
    _this: Raw,
    guid: *const Guid,
    out: *mut Raw,
) -> HResult {
    display_attribute::get_info(guid, out)
}

unsafe extern "system" fn activate(this: Raw, manager: Raw, client_id: u32) -> HResult {
    activate_with_flags(this, manager, client_id, 0)
}

unsafe extern "system" fn activate_ex(
    this: Raw,
    manager: Raw,
    client_id: u32,
    flags: u32,
) -> HResult {
    activate_with_flags(this, manager, client_id, flags)
}

unsafe fn activate_with_flags(this: Raw, manager: Raw, client_id: u32, flags: u32) -> HResult {
    if manager.is_null() {
        return ffi::E_POINTER;
    }
    let service = this as *mut TextService;
    if !(*service).key_mgr.is_null() || client_id == 0 {
        return ffi::E_UNEXPECTED;
    }
    let dict = match dictionary() {
        Ok(value) => value,
        Err(hr) => {
            diagnostics::result("tsf", "activate_dictionary", hr);
            return hr;
        }
    };
    let sandboxed = dict.is_none();
    let key_mgr = match ffi::query(manager, &ffi::IID_ITfKeystrokeMgr) {
        Ok(value) => value,
        Err(hr) => {
            diagnostics::result("tsf", "activate_keystroke_manager", hr);
            return hr;
        }
    };
    (*service).session = Some(dict.map_or_else(Session::sandboxed, Session::local));
    (*service).key_mgr = key_mgr;
    (*service).client_id = client_id;
    let sink = ptr::addr_of_mut!((*service).key_sink_vtbl).cast();
    let hr = (ffi::vtable::<KeyStrokeMgrVtbl>(key_mgr).advise)(key_mgr, client_id, sink, 1);
    let ascii = synchronize_modes(service);
    if hr < 0 {
        (*service).session = None;
        (*service).key_mgr = ptr::null_mut();
        (*service).client_id = 0;
        ffi::release(key_mgr);
    } else {
        let ui_less = flags & ffi::TF_TMAE_UIELEMENTENABLEDONLY != 0;
        (*service).candidate_ui.attach(
            manager,
            UiCallbacks {
                owner: service.cast(),
                set_selection: ui_set_selection,
                finalize: ui_finalize,
                abort: ui_abort,
                show: ui_show,
            },
            ui_less,
        );
        if ui_less && !(*service).candidate_ui.has_manager() {
            CandidateUi::detach(ptr::addr_of_mut!((*service).candidate_ui));
            let _ = (ffi::vtable::<KeyStrokeMgrVtbl>(key_mgr).unadvise)(key_mgr, client_id);
            (*service).session = None;
            (*service).key_mgr = ptr::null_mut();
            (*service).client_id = 0;
            ffi::release(key_mgr);
            diagnostics::result("tsf", "activate_ui_element_manager", ffi::E_NOINTERFACE);
            return ffi::E_NOINTERFACE;
        }
        (*service)
            .toast_ui
            .attach(manager, service.cast(), toast_show, ui_less);
        (*service)
            .language_bar
            .attach(manager, service.cast(), mode_icon_clicked, ascii);
    }
    diagnostics::event(
        if hr < 0 { Level::Error } else { Level::Info },
        "tsf",
        "activate",
        &[
            ("flags", i64::from(flags)),
            ("sandboxed", i64::from(sandboxed)),
            ("ascii_mode", i64::from(ascii)),
            ("hresult", diagnostics::hresult(hr)),
        ],
    );
    hr
}

unsafe extern "system" fn deactivate(this: Raw) -> HResult {
    let service = this as *mut TextService;
    cancel_pending(service);
    remove_mouse_hook(service);
    CandidateUi::detach(ptr::addr_of_mut!((*service).candidate_ui));
    ToastUi::detach(ptr::addr_of_mut!((*service).toast_ui));
    (*service).window.reset();
    (*service).mode_toast.reset();
    (*service).language_bar.detach();
    if (*service).composition.is_active() {
        composition::abandon(ptr::addr_of_mut!((*service).composition));
    }
    (*service).session = None;
    (*service).shift_pending = false;
    (*service).eaten_keys.fill(false);
    let key_mgr = std::mem::replace(&mut (*service).key_mgr, ptr::null_mut());
    let client_id = std::mem::replace(&mut (*service).client_id, 0);
    if key_mgr.is_null() {
        return ffi::S_OK;
    }
    // 即使宿主的 COM 仍快取 DLL，重新啟用也會重新建立本機字庫。
    #[cfg(not(test))]
    DICTIONARY
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .take();
    let hr = (ffi::vtable::<KeyStrokeMgrVtbl>(key_mgr).unadvise)(key_mgr, client_id);
    ffi::release(key_mgr);
    diagnostics::result("tsf", "deactivate", hr);
    hr
}

unsafe extern "system" fn on_set_focus(this: Raw, foreground: i32) -> HResult {
    let service = key_owner(this);
    (*service).focus_epoch = (*service).focus_epoch.wrapping_add(1);
    diagnostics::event(
        Level::Detail,
        "tsf",
        "focus",
        &[("foreground", i64::from(foreground != 0))],
    );
    if foreground == 0 {
        cancel_pending(service);
        remove_mouse_hook(service);
        CandidateUi::end(ptr::addr_of_mut!((*service).candidate_ui));
        ToastUi::end(ptr::addr_of_mut!((*service).toast_ui));
        (*service).window.reset();
        (*service).mode_toast.reset();
        (*service).shift_pending = false;
        (*service).eaten_keys.fill(false);
    } else {
        synchronize_modes(service);
    }
    ffi::S_OK
}

unsafe extern "system" fn on_test_key_down(
    this: Raw,
    context: Raw,
    key: usize,
    _flags: isize,
    eaten: *mut i32,
) -> HResult {
    if eaten.is_null() {
        return ffi::E_POINTER;
    }
    *eaten = 0;
    let service = key_owner(this);
    // 只預判按鍵。真正修改文件的工作由 OnKeyDown 執行。
    let action = observe_key_down(service, key);
    if !context.is_null() && (*service).client_id != 0 {
        if let (Some(action), Some(session)) = (action, session_for_keys(&*service)) {
            *eaten = session.accepts(action) as i32;
        }
    }
    ffi::S_OK
}

unsafe extern "system" fn on_test_key_up(
    this: Raw,
    _context: Raw,
    key: usize,
    _flags: isize,
    eaten: *mut i32,
) -> HResult {
    if eaten.is_null() {
        return ffi::E_POINTER;
    }
    let service = key_owner(this);
    *eaten = if shift_key(key) {
        ((*service).shift_pending && modifiers().can_toggle_shift()) as i32
    } else {
        (key < 256 && (*service).eaten_keys[key]) as i32
    };
    ffi::S_OK
}

unsafe extern "system" fn on_key_down(
    this: Raw,
    context: Raw,
    key: usize,
    _flags: isize,
    eaten: *mut i32,
) -> HResult {
    if eaten.is_null() {
        return ffi::E_POINTER;
    }
    *eaten = 0;
    let service = key_owner(this);
    // 有些 TSF 宿主直接呼叫 OnKeyDown／OnKeyUp，不會先試探按鍵。
    let Some(action) = observe_key_down(service, key) else {
        return ffi::S_OK;
    };
    if perform_key(service, context, action) {
        *eaten = 1;
        if key < 256 {
            (*service).eaten_keys[key] = true;
        }
    }
    ffi::S_OK
}

unsafe extern "system" fn on_key_up(
    this: Raw,
    context: Raw,
    key: usize,
    _flags: isize,
    eaten: *mut i32,
) -> HResult {
    if eaten.is_null() {
        return ffi::E_POINTER;
    }
    let service = key_owner(this);
    *eaten = 0;
    if shift_key(key) {
        let pending = std::mem::replace(&mut (*service).shift_pending, false);
        if pending {
            let _ = perform_key(service, context, Key::ToggleMode);
        }
        // Shift down 已交給應用程式，release 也必須交還，避免黏鍵。
    } else if key < 256 && (*service).eaten_keys[key] {
        (*service).eaten_keys[key] = false;
        *eaten = 1;
    }
    ffi::S_OK
}

unsafe extern "system" fn on_preserved_key(
    _this: Raw,
    _context: Raw,
    _guid: *const Guid,
    eaten: *mut i32,
) -> HResult {
    if eaten.is_null() {
        return ffi::E_POINTER;
    }
    *eaten = 0;
    ffi::S_OK
}

unsafe extern "system" fn on_composition_terminated(
    this: Raw,
    cookie: u32,
    composition_ptr: Raw,
) -> HResult {
    let service = composition_owner(this);
    if composition::terminated(
        ptr::addr_of_mut!((*service).composition),
        composition_ptr,
        cookie,
    ) {
        (*service).deferred.take();
        if let Some(session) = (*service).session.as_mut() {
            session.reset_preedit();
        }
        (*service).window.reset();
        remove_mouse_hook(service);
        CandidateUi::end(ptr::addr_of_mut!((*service).candidate_ui));
    }
    ffi::S_OK
}
