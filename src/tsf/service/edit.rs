//! TSF 編輯工作階段、多行提交及 Notepad++ 長文上屏。
use std::ffi::CString;
use std::ptr;
use std::sync::{
    atomic::{AtomicU32, Ordering},
    OnceLock,
};
use std::time::Instant;

use super::input::{discard_pending_preedit, finish_pending_preedit, is_pending_preedit};
use super::{processor_add_ref, processor_release, scintilla, TextService};
use crate::diagnostics::{self, Level, WarningLimit};
use crate::engine::Edit;
use crate::tsf::OBJECTS;
use crate::tsf::{
    composition,
    ffi::{self, ContextVtbl, EditSessionVtbl, Guid, HResult, Raw, Rect, UnknownVtbl},
};

#[repr(C)]
struct EditSession {
    vtbl: *const EditSessionVtbl,
    refs: AtomicU32,
    context: Raw,
    service: *mut TextService,
    edit: Edit,
    trace_id: u32,
    deferred: bool,
    may_run: bool,
    result: Option<Result<Option<Rect>, HResult>>,
}

pub(super) enum Preedit {
    Applied(Option<Rect>),
    Retry(u32, Edit),
    Failed,
}

struct Attempt {
    invoked: Option<Result<Option<Rect>, HResult>>,
    retry: Option<Edit>,
    queued: bool,
}

static EDIT_ID: AtomicU32 = AtomicU32::new(1);

static EDIT_VTBL: EditSessionVtbl = EditSessionVtbl {
    base: UnknownVtbl {
        query: edit_query,
        add_ref: edit_add_ref,
        release: edit_release,
    },
    do_edit_session,
};

pub(super) unsafe fn request_edit(
    service: *mut TextService,
    context: Raw,
    edit: Edit,
) -> Option<Option<Rect>> {
    let text = match edit {
        Edit::Commit(text) => text,
        Edit::CommitCandidate(text) => {
            if text.len() >= scintilla::MIN_BATCH_BYTES
                && replace_in_notepad_plus_plus(service, context, &text)
            {
                return Some(None);
            }
            text
        }
        edit => return request_edit_session(service, context, edit),
    };
    if text.contains(['\n', '\r'])
        && !is_notepad_plus_plus()
        && (*service).composition.context() == context
        && !context.is_null()
    {
        let original = (*service).composition.original()?;
        let _context = ffi::ComPtr::retain(context);
        let focus_epoch = (*service).focus_epoch;
        // 結束組字後再整段插入。富文字網頁的 DOM 在 IME compositionend 期間
        // 重新整理多個段落時，可能把換行前的字元移到文字末尾。
        if request_edit_session(service, context, Edit::PrepareMultiline).is_some() {
            let same_focus = focus_epoch == (*service).focus_epoch;
            if same_focus {
                if let Some(anchor) = request_edit_session(service, context, Edit::Insert(text)) {
                    if focus_epoch == (*service).focus_epoch {
                        let _ = request_edit_session(service, context, Edit::ConfirmMultilineCaret);
                    }
                    let correction = (*service).composition.take_caret_correction();
                    if focus_epoch == (*service).focus_epoch {
                        if let (Some(correction), Ok(root)) =
                            (correction, crate::project_files::dll_root())
                        {
                            correction.apply(&root);
                        }
                    }
                    return Some(anchor);
                }
            }
            let restored = original.is_empty()
                || request_edit_session(service, context, Edit::RestoreSelection(original))
                    .is_some();
            diagnostics::event(
                if restored {
                    Level::Warning
                } else {
                    Level::Error
                },
                "edit",
                "multiline_interrupted",
                &[
                    ("same_focus", i64::from(same_focus)),
                    ("restored", i64::from(restored)),
                ],
            );
            // 準備階段已離開組字。吸收選字按鍵，避免數字漏入文件。
            return Some(None);
        }
    }
    request_edit_session(service, context, Edit::Commit(text))
}

unsafe fn replace_in_notepad_plus_plus(
    service: *mut TextService,
    context: Raw,
    text: &str,
) -> bool {
    let composition = &(*service).composition;
    if composition.context() != context
        || !composition.original().is_some_and(|text| text.is_empty())
    {
        return false;
    }
    let Some(target) = scintilla::Selection::focused() else {
        return false;
    };
    let Ok(text) = CString::new(text) else {
        return false;
    };
    let focus_epoch = (*service).focus_epoch;
    let prepared = request_edit_session(service, context, Edit::Cancel).is_some();
    let same_focus = focus_epoch == (*service).focus_epoch;
    let same_target = scintilla::Selection::focused() == Some(target);
    if !prepared && same_focus && same_target && (*service).composition.context() == context {
        return false; // 原組字仍可使用一般 TSF 提交。
    }
    if prepared && same_focus && same_target {
        target.replace(&text);
    } else {
        diagnostics::event(
            Level::Warning,
            "edit",
            "scintilla_interrupted",
            &[
                ("prepared", i64::from(prepared)),
                ("same_focus", i64::from(same_focus)),
                ("same_target", i64::from(same_target)),
            ],
        );
    }
    true
}

// Scintilla 可原樣替換組字範圍；單次編輯避免額外的文字更新。
fn is_notepad_plus_plus() -> bool {
    static MATCHES: OnceLock<bool> = OnceLock::new();
    *MATCHES.get_or_init(|| {
        std::env::current_exe().ok().is_some_and(|path| {
            path.file_name()
                .is_some_and(|name| name.eq_ignore_ascii_case("notepad++.exe"))
        })
    })
}

unsafe fn request_edit_session(
    service: *mut TextService,
    context: Raw,
    edit: Edit,
) -> Option<Option<Rect>> {
    if context.is_null() || (*service).client_id == 0 {
        return None;
    }
    let trace_id = EDIT_ID.fetch_add(1, Ordering::Relaxed);
    submit(service, context, edit, trace_id, false)
        .invoked?
        .ok()
}

pub(super) unsafe fn request_preedit(
    service: *mut TextService,
    context: Raw,
    edit: Edit,
) -> Preedit {
    if (*service).client_id == 0 {
        return Preedit::Failed;
    }
    let id = EDIT_ID.fetch_add(1, Ordering::Relaxed);
    let attempt = submit(service, context, edit, id, false);
    match (attempt.invoked, attempt.retry) {
        (Some(Ok(anchor)), _) => Preedit::Applied(anchor),
        (None, Some(edit)) => Preedit::Retry(id, edit),
        _ => Preedit::Failed,
    }
}

pub(super) unsafe fn retry_preedit(
    service: *mut TextService,
    context: Raw,
    edit: Edit,
    trace_id: u32,
) -> bool {
    let attempt = submit(service, context, edit, trace_id, true);
    attempt
        .invoked
        .map_or(attempt.queued, |result| result.is_ok())
}

unsafe fn submit(
    service: *mut TextService,
    context: Raw,
    edit: Edit,
    trace_id: u32,
    deferred: bool,
) -> Attempt {
    static FAILED_EDIT: WarningLimit = WarningLimit::new();
    let operation = match edit {
        Edit::None => "none",
        Edit::Inspect => "inspect",
        Edit::Preedit(_) => "preedit",
        Edit::Insert(_) => "insert",
        Edit::Commit(_) | Edit::CommitCandidate(_) => "commit",
        Edit::PrepareMultiline => "prepare_multiline",
        Edit::ConfirmMultilineCaret => "confirm_multiline_caret",
        Edit::RestoreSelection(_) => "restore_selection",
        Edit::Cancel => "cancel",
        Edit::CancelExternal => "cancel_external",
    };
    let started = Instant::now();
    ffi::add_ref(context);
    processor_add_ref(service.cast());
    OBJECTS.fetch_add(1, Ordering::SeqCst);
    let session = Box::into_raw(Box::new(EditSession {
        vtbl: &EDIT_VTBL,
        refs: AtomicU32::new(1),
        context,
        service,
        edit,
        trace_id,
        deferred,
        may_run: true,
        result: None,
    }));
    let mut session_hr = ffi::E_FAIL;
    let hr = (ffi::vtable::<ContextVtbl>(context).request_edit_session)(
        context,
        (*service).client_id,
        session.cast(),
        (if deferred {
            ffi::TF_ES_ASYNC
        } else {
            ffi::TF_ES_SYNC
        }) | if (*session).edit == Edit::Inspect {
            ffi::TF_ES_READ
        } else {
            ffi::TF_ES_READWRITE
        },
        &mut session_hr,
    );
    let invoked = (*session).result;
    let queued = deferred && hr >= 0 && session_hr == ffi::TF_S_ASYNC;
    // 宿主意外延後同步要求時，丟棄舊回呼，只接受明確排程的非同步重試。
    (*session).may_run = queued && invoked.is_none();
    let retry = (!deferred
        && invoked.is_none()
        && matches!((*session).edit, Edit::Preedit(_))
        && (matches!(hr, ffi::E_UNEXPECTED | ffi::TF_E_SYNCHRONOUS)
            || matches!(session_hr, ffi::TF_S_ASYNC | ffi::TF_E_SYNCHRONOUS)))
    .then(|| std::mem::replace(&mut (*session).edit, Edit::None));
    ffi::release(session.cast());
    let ok = hr >= 0 && (queued || invoked.is_some_and(|result| result.is_ok()));
    let elapsed = started.elapsed();
    let slow = elapsed.as_millis() >= 250;
    if ok || FAILED_EDIT.ready() {
        diagnostics::event(
            if ok && !slow {
                Level::Detail
            } else {
                Level::Warning
            },
            "edit",
            operation,
            &[
                ("edit_id", i64::from(trace_id)),
                ("request_hr", diagnostics::hresult(hr)),
                ("session_hr", diagnostics::hresult(session_hr)),
                ("callback_ran", i64::from(invoked.is_some())),
                ("async", i64::from(deferred)),
                (
                    "anchor",
                    i64::from(invoked.is_some_and(|result| matches!(result, Ok(Some(_))))),
                ),
                ("elapsed_ms", diagnostics::millis(elapsed)),
            ],
        );
    }
    Attempt {
        invoked,
        retry,
        queued,
    }
}

unsafe extern "system" fn edit_query(this: Raw, iid: *const Guid, out: *mut Raw) -> HResult {
    if out.is_null() {
        return ffi::E_POINTER;
    }
    *out = ptr::null_mut();
    if iid.is_null() {
        return ffi::E_POINTER;
    }
    if *iid != ffi::IID_IUnknown && *iid != ffi::IID_ITfEditSession {
        return ffi::E_NOINTERFACE;
    }
    *out = this;
    edit_add_ref(this);
    ffi::S_OK
}
unsafe extern "system" fn edit_add_ref(this: Raw) -> u32 {
    (*(this as *mut EditSession))
        .refs
        .fetch_add(1, Ordering::Relaxed)
        + 1
}
unsafe extern "system" fn edit_release(this: Raw) -> u32 {
    let edit = this as *mut EditSession;
    let refs = (*edit).refs.fetch_sub(1, Ordering::Release) - 1;
    if refs == 0 {
        std::sync::atomic::fence(Ordering::Acquire);
        if (*edit).deferred && (*edit).result.is_none() {
            let service = (*edit).service;
            let id = (*edit).trace_id;
            if discard_pending_preedit(service, id) {
                diagnostics::event(
                    Level::Warning,
                    "edit",
                    "preedit_retry_discarded",
                    &[("edit_id", i64::from(id))],
                );
            }
        }
        ffi::release((*edit).context);
        processor_release((*edit).service.cast());
        drop(Box::from_raw(edit));
        OBJECTS.fetch_sub(1, Ordering::SeqCst);
    }
    refs
}
unsafe extern "system" fn do_edit_session(this: Raw, cookie: u32) -> HResult {
    let edit = this as *mut EditSession;
    let service = (*edit).service;
    let id = (*edit).trace_id;
    if !(*edit).may_run || ((*edit).deferred && !is_pending_preedit(service, id)) {
        return ffi::S_OK;
    }
    (*edit).may_run = false;
    let sink = ptr::addr_of_mut!((*service).composition_sink_vtbl).cast();
    let result = composition::apply(
        ptr::addr_of_mut!((*service).composition),
        (*edit).context,
        cookie,
        sink,
        &(*edit).edit,
        id,
    );
    (*edit).result = Some(result);
    let hr = result.err().unwrap_or(ffi::S_OK);
    if (*edit).deferred {
        diagnostics::event(
            if hr >= 0 { Level::Info } else { Level::Warning },
            "edit",
            "preedit_retry_complete",
            &[
                ("edit_id", i64::from(id)),
                ("hresult", diagnostics::hresult(hr)),
                ("anchor", i64::from(matches!(result, Ok(Some(_))))),
            ],
        );
        finish_pending_preedit(service, (*edit).context, id, result);
    }
    hr
}
