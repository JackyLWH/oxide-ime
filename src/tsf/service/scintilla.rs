//! Notepad++ 的相容路徑。Scintilla 會逐字處理一般 IME 結果，長文則可批量寫入。
use crate::win32::{GetAncestor, GetCurrentThreadId, GetFocus};

use std::ffi::CStr;
use std::time::Instant;

use super::ffi::Raw;
use crate::diagnostics::{self, Level};

pub(super) const MIN_BATCH_BYTES: usize = 1024;

const SCI_GETLENGTH: u32 = 2006;
const SCI_GETCURRENTPOS: u32 = 2008;
const SCI_GETANCHOR: u32 = 2009;
const SCI_GETCODEPAGE: u32 = 2137;
const SCI_GETREADONLY: u32 = 2140;
const SCI_REPLACESEL: u32 = 2170;
const SCI_GETOVERTYPE: u32 = 2187;
const SCI_GETDOCPOINTER: u32 = 2357;
const SCI_GETSELECTIONMODE: u32 = 2423;
const SCI_GETSELECTIONS: u32 = 2570;
const SCI_GETSELECTIONNCARETVIRTUALSPACE: u32 = 2581;
const SCI_GETSELECTIONNANCHORVIRTUALSPACE: u32 = 2583;
const SCI_GETIMEINTERACTION: u32 = 2678;
const SC_IME_WINDOWED: isize = 0;
const SC_SEL_STREAM: isize = 0;
const SC_CP_UTF8: isize = 65001;
const GA_ROOT: u32 = 2;

#[link(name = "user32")]
extern "system" {
    fn GetClassNameW(window: Raw, class: *mut u16, max: i32) -> i32;
    fn GetWindowThreadProcessId(window: Raw, process: *mut u32) -> u32;
    fn SendMessageW(window: Raw, message: u32, wparam: usize, lparam: isize) -> isize;
}

unsafe fn has_class(window: Raw, class: &str) -> bool {
    let mut name = [0; 32];
    let len = GetClassNameW(window, name.as_mut_ptr(), name.len() as i32);
    len > 0
        && name[..len as usize]
            .iter()
            .copied()
            .eq(class.encode_utf16())
}

unsafe fn query(window: Raw, message: u32) -> isize {
    SendMessageW(window, message, 0, 0)
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) struct Selection {
    window: Raw,
    document: isize,
    length: isize,
    caret: isize,
    anchor: isize,
}

impl Selection {
    pub(super) unsafe fn focused() -> Option<Self> {
        let window = GetFocus();
        if window.is_null()
            || GetWindowThreadProcessId(window, std::ptr::null_mut()) != GetCurrentThreadId()
            || !has_class(window, "Scintilla")
            || !has_class(GetAncestor(window, GA_ROOT), "Notepad++")
        {
            return None;
        }
        // 視窗式 IME 的預編輯在文件外部。也確認批量取代符合原本的單一選取範圍。
        if query(window, SCI_GETIMEINTERACTION) != SC_IME_WINDOWED
            || query(window, SCI_GETCODEPAGE) != SC_CP_UTF8
            || query(window, SCI_GETSELECTIONMODE) != SC_SEL_STREAM
            || query(window, SCI_GETSELECTIONS) != 1
            || query(window, SCI_GETSELECTIONNCARETVIRTUALSPACE) != 0
            || query(window, SCI_GETSELECTIONNANCHORVIRTUALSPACE) != 0
            || query(window, SCI_GETREADONLY) != 0
            || query(window, SCI_GETOVERTYPE) != 0
        {
            return None;
        }
        let document = query(window, SCI_GETDOCPOINTER);
        let length = query(window, SCI_GETLENGTH);
        let caret = query(window, SCI_GETCURRENTPOS);
        let anchor = query(window, SCI_GETANCHOR);
        (document != 0 && (0..=length).contains(&caret) && (0..=length).contains(&anchor))
            .then_some(Self {
                window,
                document,
                length,
                caret,
                anchor,
            })
    }

    /// 在同一執行緒同步傳送；Scintilla 自行將取代合併成一次復原。
    pub(super) unsafe fn replace(self, text: &CStr) {
        let started = Instant::now();
        SendMessageW(self.window, SCI_REPLACESEL, 0, text.as_ptr() as isize);
        let bytes = text.to_bytes().len() as isize;
        let remaining = self.length - (self.caret - self.anchor).abs();
        let length = query(self.window, SCI_GETLENGTH);
        let complete = query(self.window, SCI_GETDOCPOINTER) == self.document
            && remaining.checked_add(bytes) == Some(length);
        let elapsed = started.elapsed();
        diagnostics::event(
            if !complete {
                Level::Error
            } else if elapsed.as_millis() >= 250 {
                Level::Warning
            } else {
                Level::Detail
            },
            "edit",
            "scintilla_replace",
            &[
                ("byte_count", bytes as i64),
                ("elapsed_ms", diagnostics::millis(elapsed)),
                ("expected_length", i64::from(complete)),
            ],
        );
    }
}
