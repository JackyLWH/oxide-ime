//! 對應 TSF 的宿主繪製介面、原生視窗及滑鼠選字。
use std::ptr;

use super::super::{
    ffi::{Raw, Rect},
    ui_element::{CandidateUi, ToastUi},
};
use super::{edit::request_edit, input::perform_key, TextService};
use crate::engine::{Edit, Key, PAGE_SIZE};

unsafe fn render_window(service: *mut TextService, anchor: Option<Rect>) {
    let Some(session) = (*service).session.as_ref() else {
        (*service).window.reset();
        return;
    };
    if session.menu_open {
        (*service)
            .window
            .update_switches(&session.switch_items(), anchor);
    } else {
        (*service)
            .window
            .update(&session.candidates(), session.page(), anchor);
    }
    (*service)
        .mode_toast
        .reposition_toast((*service).window.visible_rect());
}

pub(super) unsafe fn update_window(service: *mut TextService, anchor: Option<Rect>, context: Raw) {
    let candidate_ui = ptr::addr_of_mut!((*service).candidate_ui);
    let Some(snapshot) = (*service)
        .session
        .as_ref()
        .filter(|session| session.ui_count() != 0)
        .cloned()
    else {
        CandidateUi::end(candidate_ui);
        if (*service).session.is_some() {
            (*service).window.hide();
        } else {
            (*service).window.reset();
        }
        return;
    };
    let show = CandidateUi::update(candidate_ui, snapshot, context);
    if show {
        render_window(service, anchor);
    } else {
        (*service).window.hide();
    }
}

pub(super) unsafe fn show_mode_toast(
    service: *mut TextService,
    label: &'static str,
    anchor: Option<Rect>,
    context: Raw,
) {
    (*service).mode_toast.hide();
    (*service).toast_label = label;
    (*service).toast_anchor = anchor;
    if ToastUi::begin(ptr::addr_of_mut!((*service).toast_ui), label, context) {
        (*service)
            .mode_toast
            .show_toast(label, anchor, (*service).window.visible_rect());
    } else {
        (*service).mode_toast.start_mode_timer();
    }
}

pub(super) unsafe fn mode_icon_clicked(owner: Raw) {
    let service = owner as *mut TextService;
    let context = current_context(service);
    let _ = perform_key(service, context, Key::ToggleMode);
}

pub(super) unsafe fn candidate_clicked(owner: Raw, index: usize) {
    let service = owner as *mut TextService;
    let context = current_context(service);
    let _ = perform_key(service, context, Key::Select(index));
}

unsafe fn current_context(service: *mut TextService) -> Raw {
    let context = (*service).composition.context();
    if context.is_null() {
        CandidateUi::context(ptr::addr_of!((*service).candidate_ui))
    } else {
        context
    }
}

pub(super) unsafe fn ui_set_selection(owner: Raw, index: usize) -> bool {
    let service = owner as *mut TextService;
    let Some(mut next) = (*service).session.clone() else {
        return false;
    };
    let Some(edit) = next.ui_set_selection(index) else {
        return false;
    };
    let mut anchor = None;
    if edit != Edit::None {
        let context = (*service).composition.context();
        let Some(result) = request_edit(service, context, edit) else {
            return false;
        };
        anchor = result;
    }
    (*service).session = Some(next.clone());
    CandidateUi::replace_snapshot(ptr::addr_of_mut!((*service).candidate_ui), next);
    if CandidateUi::show_allowed(ptr::addr_of!((*service).candidate_ui)) {
        render_window(service, anchor);
    }
    true
}

pub(super) unsafe fn ui_finalize(owner: Raw) -> bool {
    let service = owner as *mut TextService;
    let Some(session) = (*service).session.as_ref() else {
        return false;
    };
    let Some(index) = session.ui_selection() else {
        return false;
    };
    let position = if session.menu_open {
        index
    } else {
        index % PAGE_SIZE
    };
    let context = current_context(service);
    perform_key(service, context, Key::Select(position))
}

pub(super) unsafe fn ui_abort(owner: Raw) -> bool {
    let service = owner as *mut TextService;
    let context = current_context(service);
    perform_key(service, context, Key::Escape)
}

pub(super) unsafe fn ui_show(owner: Raw, show: bool) {
    let service = owner as *mut TextService;
    if show {
        render_window(service, None);
    } else {
        (*service).window.hide();
    }
}

pub(super) unsafe fn toast_show(owner: Raw, show: bool) {
    let service = owner as *mut TextService;
    if show {
        (*service).mode_toast.show_toast(
            (*service).toast_label,
            (*service).toast_anchor,
            (*service).window.visible_rect(),
        );
    } else {
        (*service).mode_toast.hide();
        (*service).mode_toast.start_mode_timer();
    }
}

pub(super) unsafe fn toast_expired(owner: Raw) {
    let service = owner as *mut TextService;
    ToastUi::end(ptr::addr_of_mut!((*service).toast_ui));
}
