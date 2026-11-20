//! 候選與狀態提示透過 TSF UIElement 協調顯示。
mod candidates;
mod toast;

use super::ffi::{self, ComPtr, Raw, UiElementMgrVtbl};
use crate::diagnostics::{self, Level, WarningLimit};
pub(super) use candidates::{Callbacks, CandidateUi};
pub(super) use toast::ToastUi;

#[derive(Default)]
struct Registration {
    manager: Option<ComPtr>,
    id: Option<u32>,
    require_manager: bool,
}

impl Registration {
    unsafe fn attach(&mut self, manager: Raw, require_manager: bool) {
        self.manager = match ComPtr::query(manager, &ffi::IID_ITfUIElementMgr) {
            Ok(value) => Some(value),
            Err(hr) => {
                diagnostics::event(
                    if require_manager {
                        Level::Warning
                    } else {
                        Level::Detail
                    },
                    "ui_element",
                    "manager_unavailable",
                    &[("hresult", diagnostics::hresult(hr))],
                );
                None
            }
        };
        self.require_manager = require_manager;
    }

    fn has_manager(&self) -> bool {
        self.manager.is_some()
    }

    fn fallback_show(&self) -> bool {
        !self.require_manager
    }

    fn manager(&self) -> Option<Raw> {
        self.manager.as_ref().map(ComPtr::as_raw)
    }

    // TSF 可能同步回呼 Show，因而使用原始指標跨越 COM 呼叫。
    unsafe fn begin(this: *mut Self, element: Raw) -> bool {
        let Some(manager) = (*this).manager() else {
            return (*this).fallback_show();
        };
        let mut show = 1;
        let mut id = 0;
        let hr =
            (ffi::vtable::<UiElementMgrVtbl>(manager).begin)(manager, element, &mut show, &mut id);
        diagnostics::event(
            if hr < 0 {
                Level::Warning
            } else {
                Level::Detail
            },
            "ui_element",
            "begin",
            &[
                ("hresult", diagnostics::hresult(hr)),
                ("show_native", i64::from(show != 0)),
            ],
        );
        if hr < 0 {
            return (*this).fallback_show();
        }
        (*this).id = Some(id);
        show != 0
    }

    unsafe fn update(this: *const Self) {
        if let (Some(manager), Some(id)) = ((*this).manager(), (*this).id) {
            let hr = (ffi::vtable::<UiElementMgrVtbl>(manager).update)(manager, id);
            static UPDATE_FAILED: WarningLimit = WarningLimit::new();
            if hr < 0 && UPDATE_FAILED.ready() {
                diagnostics::event(
                    Level::Warning,
                    "ui_element",
                    "update_failed",
                    &[("hresult", diagnostics::hresult(hr))],
                );
            }
        }
    }

    unsafe fn end(this: *mut Self) {
        if let (Some(manager), Some(id)) = ((*this).manager(), (*this).id.take()) {
            let _ = (ffi::vtable::<UiElementMgrVtbl>(manager).end)(manager, id);
        }
    }
}
