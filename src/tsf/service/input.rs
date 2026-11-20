//! 由按鍵決定組字狀態、編輯鎖重試及共用模式。
use std::collections::VecDeque;
use std::ptr;

use super::super::{
    ffi::{ComPtr, HResult, Raw, Rect},
    language_bar::mode_label,
    ui_element::CandidateUi,
};
use super::{
    dictionary,
    edit::{self, request_edit},
    keys::{modifiers, shift_key, translate_key},
    mouse::{install_mouse_hook, remove_mouse_hook},
    presentation::{show_mode_toast, update_window},
    TextService, DICTIONARY,
};
use crate::broker::{self, ModeAction};
use crate::diagnostics::{self, Level};
use crate::engine::{Edit, Key, Session};
use crate::win32::{GetFocus, GetForegroundWindow};

pub(super) const DISMISS_MENU: usize = 1;
const REPLAY_KEYS: usize = 2;

pub(super) struct DeferredInput {
    id: u32,
    context: ComPtr,
    focus_epoch: u32,
    focus_window: Raw,
    foreground: Raw,
    transition: Option<(Key, Session, KeyState)>,
    keys: VecDeque<Key>,
}

impl DeferredInput {
    unsafe fn has_focus(&self, service: *const TextService) -> bool {
        self.focus_epoch == (*service).focus_epoch
            && self.focus_window == GetFocus()
            && self.foreground == GetForegroundWindow()
    }
}

struct KeyState {
    ascii: bool,
    simplified: bool,
    startup: bool,
    menu: bool,
    reverse: Option<bool>,
    composing: bool,
}

pub(super) unsafe fn cancel_pending(service: *mut TextService) -> bool {
    let had_deferred = (*service).deferred.take().is_some();
    let context = (*service).composition.context();
    if (*service).composition.is_active()
        && request_edit(service, context, Edit::CancelExternal).is_none()
    {
        return false;
    }
    let had_menu = (*service).session.as_mut().is_some_and(|session| {
        session.reset_preedit();
        session.close_menu()
    });
    if !context.is_null() || had_menu || had_deferred {
        remove_mouse_hook(service);
        CandidateUi::end(ptr::addr_of_mut!((*service).candidate_ui));
        (*service).window.reset();
    }
    true
}

pub(super) unsafe fn cancel_from_click(service: *mut TextService) {
    let context = (*service).composition.context();
    let jyutping = !context.is_null()
        && (*service)
            .session
            .as_ref()
            .is_some_and(|session| session.reverse_exact().is_some());
    let context = jyutping.then(|| ComPtr::retain(context));
    if cancel_pending(service) {
        if let Some(context) = context {
            perform_key(service, context.as_raw(), Key::Escape);
        }
    }
}

pub(super) unsafe fn perform_key(service: *mut TextService, context: Raw, key: Key) -> bool {
    // 滑鼠選字或 TSF UI 回呼提供的 context 也必須跨過候選與組字結束。
    let _context = (!context.is_null()).then(|| ComPtr::retain(context));
    while let Some(pending) = (*service).deferred.as_mut() {
        if pending.context.as_raw() != context || !pending.has_focus(service) {
            (*service).deferred.take();
            break;
        } else if pending.transition.is_some() {
            // 組字寫入排隊時，保留同一文件後續的選字或按鍵順序。
            if pending.keys.len() < 64 {
                pending.keys.push_back(key);
                return true;
            }
            (*service).deferred.take();
            diagnostics::event(Level::Warning, "edit", "preedit_queue_full", &[]);
            break;
        } else {
            let id = pending.id;
            replay_keys(service, id);
        }
    }
    if !context.is_null()
        && (*service).composition.is_active()
        && (*service).composition.context() != context
        && !cancel_pending(service)
    {
        return false;
    }
    if key == Key::ToggleMode {
        synchronize_modes(service);
    }
    let Some(mut next) = (*service).session.clone() else {
        return false;
    };
    if !next.is_composing() {
        next.refresh_converter();
    }
    // 選字時先提交使用者已看見的候選；下一次輸入再採用新版字庫。
    if !next.is_composing() || !matches!(key, Key::Select(_) | Key::Space) {
        if let Ok(Some(updated)) = dictionary() {
            next.update_dictionaries(updated);
        }
    }
    if next.needs_local_phonetics(key) {
        let mut cache = DICTIONARY.lock().unwrap_or_else(|error| error.into_inner());
        if let Some(store) = cache.as_mut() {
            next.set_fuzzy(store.fuzzy());
        }
    }
    let previous = KeyState {
        reverse: next.reverse_exact(),
        ascii: next.ascii_mode,
        simplified: next.simplification,
        startup: next.startup_ascii,
        menu: next.menu_open,
        composing: (*service).composition.is_active(),
    };
    let outcome = next.handle(key);
    if !outcome.handled {
        return false;
    }
    let anchor = match outcome.edit {
        Edit::None if next.menu_open && !context.is_null() => {
            request_edit(service, context, Edit::Inspect).flatten()
        }
        Edit::None => None,
        _ if context.is_null() => return false,
        preedit @ Edit::Preedit(_) => {
            let focus_epoch = (*service).focus_epoch;
            let focus_window = GetFocus();
            let foreground = GetForegroundWindow();
            match edit::request_preedit(service, context, preedit) {
                edit::Preedit::Applied(anchor) => anchor,
                edit::Preedit::Retry(id, preedit)
                    if (*service).focus_epoch == focus_epoch
                        && GetFocus() == focus_window
                        && GetForegroundWindow() == foreground =>
                {
                    (*service).deferred = Some(DeferredInput {
                        id,
                        context: ComPtr::retain(context),
                        focus_epoch,
                        focus_window,
                        foreground,
                        transition: Some((key, next, previous)),
                        keys: VecDeque::new(),
                    });
                    let accepted = edit::retry_preedit(service, context, preedit, id);
                    update_mouse_watch(service);
                    return accepted;
                }
                _ => return false,
            }
        }
        edit => {
            let Some(anchor) = request_edit(service, context, edit) else {
                return false;
            };
            anchor
        }
    };
    complete_key(service, context, key, next, previous, anchor, true);
    true
}

unsafe fn complete_key(
    service: *mut TextService,
    context: Raw,
    key: Key,
    mut next: Session,
    previous: KeyState,
    mut anchor: Option<Rect>,
    inspect_anchor: bool,
) {
    if (!previous.composing && next.is_composing()) || (!previous.menu && next.menu_open) {
        (*service).window.reset();
    }
    let shared_change = if previous.ascii != next.ascii_mode {
        Some(ModeAction::ToggleAscii)
    } else if previous.simplified != next.simplification {
        Some(ModeAction::ToggleSimplification)
    } else if previous.startup != next.startup_ascii {
        Some(ModeAction::ToggleStartup)
    } else if previous.reverse != next.reverse_exact() {
        Some(ModeAction::for_reverse_mode(next.reverse_exact()))
    } else {
        None
    };
    let mut shared_ascii_changed = false;
    if let Some(action) = shared_change {
        let result = broker::modes(action);
        let shared = result.is_ok();
        shared_ascii_changed = shared && matches!(action, ModeAction::ToggleAscii);
        if let Ok(modes) = result {
            next.set_modes(modes);
        }
        diagnostics::event(
            if shared { Level::Info } else { Level::Warning },
            "tsf",
            "mode_change",
            &[
                ("action", action as i64),
                ("ascii_mode", i64::from(next.ascii_mode)),
                ("simplified", i64::from(next.simplification)),
                ("startup_ascii", i64::from(next.startup_ascii)),
                ("jyutping", i64::from(next.reverse_exact().is_some())),
                ("shared", i64::from(shared)),
            ],
        );
    }
    let reverse = next.reverse_exact();
    let returned_to_cangjie = previous.reverse.is_some() && reverse.is_none() && !next.ascii_mode;
    let new_ascii = next.ascii_mode;
    let new_simplification = next.simplification;
    (*service).session = Some(next);
    if previous.ascii != new_ascii {
        (*service).language_bar.set_ascii(new_ascii);
    }
    if shared_ascii_changed {
        (*service)
            .language_bar
            .broadcast_ascii((*service).client_id, new_ascii);
    }
    let toast_label = if previous.ascii != new_ascii && key == Key::ToggleMode {
        Some(mode_label(new_ascii))
    } else if previous.simplified != new_simplification && key == Key::ToggleSimplification {
        Some(if new_simplification { "簡" } else { "繁" })
    } else if key == Key::ToggleJyutping && previous.reverse.is_none() && reverse.is_some() {
        Some("粵拼模式")
    } else if returned_to_cangjie {
        Some("倉頡模式")
    } else if key == Key::ToggleReverseExact && previous.reverse != reverse {
        Some(if reverse == Some(true) {
            "精確拼音"
        } else {
            "模糊拼音"
        })
    } else {
        None
    };
    if toast_label.is_some() && anchor.is_none() && inspect_anchor && !context.is_null() {
        anchor = request_edit(service, context, Edit::Inspect).flatten();
    }
    update_window(service, anchor, context);
    if let Some(label) = toast_label {
        show_mode_toast(service, label, anchor, context);
    }
    update_mouse_watch(service);
}

unsafe fn update_mouse_watch(service: *mut TextService) {
    let menu = (*service)
        .session
        .as_ref()
        .is_some_and(|session| session.menu_open)
        && (*service).window.visible_rect().is_some();
    if menu || (*service).composition.is_active() || (*service).deferred.is_some() {
        install_mouse_hook(service, menu);
    } else {
        remove_mouse_hook(service);
    }
}

pub(super) unsafe fn window_event(owner: Raw, kind: usize, token: isize) {
    let service = owner as *mut TextService;
    match kind {
        DISMISS_MENU if (*service).menu_mouse_hook == token as Raw && token != 0 => {
            cancel_pending(service);
        }
        REPLAY_KEYS => replay_keys(service, token as u32),
        _ => {}
    }
}

unsafe fn replay_keys(service: *mut TextService, id: u32) {
    if (*service)
        .deferred
        .as_ref()
        .is_some_and(|pending| id == pending.id && pending.transition.is_none())
    {
        let pending = (*service).deferred.take().expect("已檢查等候中的按鍵");
        for &key in &pending.keys {
            if !pending.has_focus(service) {
                break;
            }
            perform_key(service, pending.context.as_raw(), key);
        }
    }
}

pub(super) unsafe fn is_pending_preedit(service: *mut TextService, id: u32) -> bool {
    (*service).deferred.as_ref().is_some_and(|pending| {
        id == pending.id && pending.transition.is_some() && pending.has_focus(service)
    })
}

pub(super) unsafe fn discard_pending_preedit(service: *mut TextService, id: u32) -> bool {
    if (*service)
        .deferred
        .as_ref()
        .is_some_and(|pending| id == pending.id && pending.transition.is_some())
    {
        (*service).deferred.take();
        update_mouse_watch(service);
        true
    } else {
        false
    }
}

pub(super) unsafe fn finish_pending_preedit(
    service: *mut TextService,
    context: Raw,
    id: u32,
    result: Result<Option<Rect>, HResult>,
) {
    if !is_pending_preedit(service, id) {
        return;
    }
    let mut pending = (*service).deferred.take().expect("已檢查等候中的組字");
    let (key, next, previous) = pending.transition.take().expect("等候中的組字");
    if let Ok(anchor) = result {
        complete_key(service, context, key, next, previous, anchor, false);
        if !pending.keys.is_empty() && pending.has_focus(service) {
            (*service).deferred = Some(pending);
            if (*service).window.ensure_window() {
                (*service).window.post_event(REPLAY_KEYS, id as isize);
            }
        }
    } else {
        update_mouse_watch(service);
    }
}

pub(super) unsafe fn synchronize_modes(service: *mut TextService) -> bool {
    let Some(session) = (*service).session.as_mut() else {
        return true;
    };
    // 已顯示的候選跟組字保留一組模式。新焦點及下一次組字採用共用狀態。
    if !session.is_composing() {
        if let Ok(modes) = broker::modes(ModeAction::Read) {
            session.set_modes(modes);
        }
    }
    let ascii = session.ascii_mode;
    (*service).language_bar.set_ascii(ascii);
    ascii
}

pub(super) unsafe fn observe_key_down(service: *mut TextService, key: usize) -> Option<Key> {
    let mods = modifiers();
    if shift_key(key) {
        (*service).shift_pending = mods.can_toggle_shift();
        return None;
    }
    (*service).shift_pending = false;
    synchronize_modes(service);
    let session = session_for_keys(&*service)?;
    translate_key(
        key,
        mods,
        session.is_composing() || (session.reverse_exact().is_some() && !session.menu_open),
        session.menu_open,
    )
}

pub(super) fn session_for_keys(service: &TextService) -> Option<&Session> {
    service
        .deferred
        .as_ref()
        .and_then(|pending| pending.transition.as_ref().map(|(_, next, _)| next))
        .or(service.session.as_ref())
}
