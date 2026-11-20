//! Chromium 多行插入的游標相容處理。只在原文吻合時調整插入點。
use std::io::{Read, Write};
use std::process::Stdio;
use std::ptr;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::diagnostics::{self, Level};
use crate::tsf::ffi::{self, ComPtr, Guid, HResult, Raw, UnknownVtbl};
use crate::win32::{GetAncestor, GetFocus, GetForegroundWindow};

const START: i32 = 0;
const END: i32 = 1;
const CHARACTER: i32 = 0;
const MAX_AGE_MS: u64 = 800;
const MAX_TRAILING: usize = 64;

#[link(name = "user32")]
extern "system" {
    fn GetClassNameW(window: Raw, name: *mut u16, max: i32) -> i32;
    fn GetWindowThreadProcessId(window: Raw, pid: *mut u32) -> u32;
}

#[link(name = "kernel32")]
extern "system" {
    fn GetTickCount64() -> u64;
}

#[link(name = "oleaut32")]
extern "system" {
    fn SysStringLen(value: Raw) -> u32;
    fn SysFreeString(value: Raw);
}

#[repr(C)]
struct Automation {
    base: UnknownVtbl,
    compare_elements: unsafe extern "system" fn(Raw, Raw, Raw, *mut i32) -> HResult,
    before_focused: [usize; 4],
    focused: unsafe extern "system" fn(Raw, *mut Raw) -> HResult,
    before_raw_walker: [usize; 7],
    raw_walker: unsafe extern "system" fn(Raw, *mut Raw) -> HResult,
}

#[repr(C)]
struct Walker {
    base: UnknownVtbl,
    parent: unsafe extern "system" fn(Raw, Raw, *mut Raw) -> HResult,
}

#[repr(C)]
struct Element {
    base: UnknownVtbl,
    before_pattern: [usize; 11],
    get_pattern: unsafe extern "system" fn(Raw, i32, *const Guid, *mut Raw) -> HResult,
    before_process: [usize; 5],
    process: unsafe extern "system" fn(Raw, *mut u32) -> HResult,
}

#[repr(C)]
struct TextPattern {
    base: UnknownVtbl,
    before_selection: [usize; 2],
    get_selection: unsafe extern "system" fn(Raw, *mut Raw) -> HResult,
}

#[repr(C)]
struct TextRangeArray {
    base: UnknownVtbl,
    length: unsafe extern "system" fn(Raw, *mut i32) -> HResult,
    element: unsafe extern "system" fn(Raw, i32, *mut Raw) -> HResult,
}

#[repr(C)]
struct TextRange {
    base: UnknownVtbl,
    clone: unsafe extern "system" fn(Raw, *mut Raw) -> HResult,
    compare: unsafe extern "system" fn(Raw, Raw, *mut i32) -> HResult,
    compare_endpoints: unsafe extern "system" fn(Raw, i32, Raw, i32, *mut i32) -> HResult,
    before_text: [usize; 6],
    get_text: unsafe extern "system" fn(Raw, i32, *mut *mut u16) -> HResult,
    move_range: usize,
    move_by_unit: unsafe extern "system" fn(Raw, i32, i32, i32, *mut i32) -> HResult,
    move_by_range: unsafe extern "system" fn(Raw, i32, Raw, i32) -> HResult,
    select: unsafe extern "system" fn(Raw) -> HResult,
}

#[derive(Deserialize, Serialize)]
pub(crate) struct Correction {
    window: usize,
    process: u32,
    created_ms: u64,
    before: String,
    remainder: String,
    following: String,
}

fn chromium_window(window: Raw) -> bool {
    let mut class = [0; 64];
    let length = unsafe { GetClassNameW(window, class.as_mut_ptr(), class.len() as i32) };
    length > 0
        && String::from_utf16_lossy(&class[..length as usize]).starts_with("Chrome_WidgetWin_")
}

impl Correction {
    pub(crate) fn prepare(text: &str) -> Option<Self> {
        let root = unsafe { GetAncestor(GetFocus(), 2) }; // GA_ROOT
        if root.is_null() || root != unsafe { GetForegroundWindow() } || !chromium_window(root) {
            return None;
        }
        let (before, remainder) = suspected_boundary(text)?;
        let mut process = 0;
        unsafe { GetWindowThreadProcessId(root, &mut process) };
        (process != 0).then_some(Self {
            window: root as usize,
            process,
            created_ms: unsafe { GetTickCount64() },
            before,
            remainder,
            following: String::new(),
        })
    }

    pub(crate) fn following(&mut self, text: String) {
        self.following = text;
    }

    pub(crate) fn apply(self, root: &std::path::Path) {
        if !self.is_current() {
            return;
        }
        let exe = crate::project_files::maintenance(root);
        let Ok(mut child) = crate::win32::hidden_command(exe)
            .arg("--correct-caret")
            .stdin(Stdio::piped())
            .spawn()
        else {
            return;
        };
        if let Some(mut pipe) = child.stdin.take() {
            if let Ok(job) = serde_json::to_vec(&self) {
                let _ = pipe.write_all(&job);
            }
        }
    }

    fn is_current(&self) -> bool {
        let window = self.window as Raw;
        let now = unsafe { GetTickCount64() };
        if now.saturating_sub(self.created_ms) > MAX_AGE_MS
            || unsafe { GetForegroundWindow() } != window
            || !chromium_window(window)
        {
            return false;
        }
        let mut process = 0;
        unsafe { GetWindowThreadProcessId(window, &mut process) };
        process != 0 && process == self.process
    }
}

fn suspected_boundary(text: &str) -> Option<(String, String)> {
    // Blink 用提交字串長度還原游標，段落分隔符在部分富文字欄佔兩個文字位置。
    let breaks = text
        .as_bytes()
        .iter()
        .enumerate()
        .filter(|(i, unit)| {
            **unit == b'\n' || (**unit == b'\r' && text.as_bytes().get(i + 1) != Some(&b'\n'))
        })
        .count();
    if !(1..=32).contains(&breaks) {
        return None;
    }
    let last = text.rsplit(['\n', '\r']).next()?;
    let mut units = 0;
    let mut tails = last.char_indices().rev();
    let (split, _) = tails.find(|(_, ch)| {
        units += ch.len_utf16();
        units >= breaks
    })?;
    if units != breaks {
        return None;
    }
    let start = tails.take(MAX_TRAILING).last()?.0;
    let (before, remainder) = (&last[start..split], &last[split..]);
    (before.chars().count() >= 8).then(|| (before.to_owned(), remainder.to_owned()))
}

pub(super) fn run() -> Result<(), String> {
    let mut bytes = Vec::new();
    std::io::stdin()
        .take(2048)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    let job: Correction = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
    if !job.is_current() || job.before.is_empty() || job.remainder.is_empty() {
        return Ok(());
    }
    unsafe {
        let hr = ffi::CoInitializeEx(ptr::null_mut(), 0); // MTA
        if hr < 0 {
            return Err(format!("無法初始化 UI Automation（0x{:08X}）", hr as u32));
        }
        let result = correct(&job);
        ffi::CoUninitialize();
        #[cfg(feature = "integration-fixtures")]
        eprintln!("chromium_caret_checked={result}");
        diagnostics::event(
            Level::Detail,
            "edit",
            "chromium_caret_checked",
            &[("result", i64::from(result))],
        );
    }
    Ok(())
}

unsafe fn correct(job: &Correction) -> i32 {
    let clsid = Guid::new(
        0xff48dba4,
        0x60ef,
        0x4201,
        [0xaa, 0x87, 0x54, 0x10, 0x3e, 0xef, 0x59, 0x4e],
    );
    let iid = Guid::new(
        0x30cbe57d,
        0xd9d0,
        0x452a,
        [0xab, 0x13, 0x7a, 0xc5, 0xac, 0x48, 0x25, 0xee],
    );
    let Ok(automation) = ffi::create_instance(&clsid, &iid) else {
        return 0;
    };
    let automation = automation.as_raw();
    let vt = ffi::vtable::<Automation>(automation);
    let mut walker = ptr::null_mut();
    let hr = (vt.raw_walker)(automation, &mut walker);
    let walker = ComPtr::from_out(hr, walker).ok();
    let mut reached = 1;
    // Chromium 有機會在第一個 UI Automation 要求後才建立無障礙樹。
    while job.is_current() {
        if let Some(focus) = focused(automation, vt, job.process) {
            reached = reached.max(2);
            if let Some(pattern) = text_pattern(&focus, walker.as_ref(), job.process) {
                reached = reached.max(3);
                if let Some(caret) = current_caret(pattern.as_raw()) {
                    reached = reached.max(4);
                    if let Some(range) = matching_tail(&caret, job) {
                        if !still_selected(automation, vt, &focus, &pattern, &caret, job) {
                            return 5;
                        }
                        let raw = range.as_raw();
                        return if (ffi::vtable::<TextRange>(raw).select)(raw) >= 0 {
                            7
                        } else {
                            6
                        };
                    }
                }
            }
        }
        std::thread::sleep(Duration::from_millis(45));
    }
    reached
}

unsafe fn focused(automation: Raw, vt: &Automation, process: u32) -> Option<ComPtr> {
    let mut element = ptr::null_mut();
    let hr = (vt.focused)(automation, &mut element);
    let element = ComPtr::from_out(hr, element).ok()?;
    same_process(element.as_raw(), process).then_some(element)
}

unsafe fn same_process(element: Raw, process: u32) -> bool {
    let mut pid = 0;
    (ffi::vtable::<Element>(element).process)(element, &mut pid) >= 0 && pid == process
}

unsafe fn text_pattern(focused: &ComPtr, walker: Option<&ComPtr>, process: u32) -> Option<ComPtr> {
    let iid = Guid::new(
        0x32eba289,
        0x3583,
        0x42c9,
        [0x9c, 0x59, 0x3b, 0x6d, 0x9a, 0x1e, 0x9b, 0x6a],
    );
    let mut element = ComPtr::retain(focused.as_raw());
    // 部分 Chrome 編輯區將 TextPattern 提供在上層文件節點。
    for _ in 0..5 {
        let raw = element.as_raw();
        let ev = ffi::vtable::<Element>(raw);
        let mut pattern = ptr::null_mut();
        let hr = (ev.get_pattern)(raw, 10014, &iid, &mut pattern); // UIA_TextPatternId
        if let Ok(pattern) = ComPtr::from_out(hr, pattern) {
            return Some(pattern);
        }
        let walker = walker?.as_raw();
        let mut parent = ptr::null_mut();
        let hr = (ffi::vtable::<Walker>(walker).parent)(walker, raw, &mut parent);
        element = ComPtr::from_out(hr, parent).ok()?;
        if !same_process(element.as_raw(), process) {
            return None;
        }
    }
    None
}

unsafe fn still_selected(
    automation: Raw,
    vt: &Automation,
    original: &ComPtr,
    pattern: &ComPtr,
    caret: &ComPtr,
    job: &Correction,
) -> bool {
    let Some(current) = focused(automation, vt, job.process) else {
        return false;
    };
    let mut same = 0;
    if (vt.compare_elements)(automation, original.as_raw(), current.as_raw(), &mut same) < 0
        || same == 0
    {
        return false;
    }
    let Some(selection) = current_caret(pattern.as_raw()) else {
        return false;
    };
    let raw = selection.as_raw();
    let mut unchanged = 0;
    (ffi::vtable::<TextRange>(raw).compare)(raw, caret.as_raw(), &mut unchanged) >= 0
        && unchanged != 0
        && job.is_current()
}

unsafe fn current_caret(pattern: Raw) -> Option<ComPtr> {
    let mut array = ptr::null_mut();
    let hr = (ffi::vtable::<TextPattern>(pattern).get_selection)(pattern, &mut array);
    let array = ComPtr::from_out(hr, array).ok()?;
    let raw = array.as_raw();
    let vt = ffi::vtable::<TextRangeArray>(raw);
    let mut count = 0;
    if (vt.length)(raw, &mut count) < 0 || count != 1 {
        return None;
    }
    let mut range = ptr::null_mut();
    let hr = (vt.element)(raw, 0, &mut range);
    let range = ComPtr::from_out(hr, range).ok()?;
    let raw = range.as_raw();
    let mut span = 0;
    ((ffi::vtable::<TextRange>(raw).compare_endpoints)(raw, START, raw, END, &mut span) >= 0
        && span == 0)
        .then_some(range)
}

unsafe fn extend(caret: &ComPtr, before: i32, after: i32) -> Option<ComPtr> {
    let current = caret.as_raw();
    let vt = ffi::vtable::<TextRange>(current);
    let mut result = ptr::null_mut();
    let hr = (vt.clone)(current, &mut result);
    let range = ComPtr::from_out(hr, result).ok()?;
    let raw = range.as_raw();
    let mut moved = 0;
    if (vt.move_by_unit)(raw, START, CHARACTER, -before, &mut moved) < 0
        || moved != -before
        || (after != 0
            && ((vt.move_by_unit)(raw, END, CHARACTER, after, &mut moved) < 0 || moved != after))
    {
        return None;
    }
    Some(range)
}

unsafe fn matches_text(range: &ComPtr, job: &Correction) -> bool {
    let raw = range.as_raw();
    let mut text = ptr::null_mut();
    let hr = (ffi::vtable::<TextRange>(raw).get_text)(raw, -1, &mut text);
    if hr < 0 || text.is_null() {
        return false;
    }
    let actual = std::slice::from_raw_parts(text, SysStringLen(text.cast()) as usize);
    let matches = actual.iter().copied().eq(job
        .before
        .encode_utf16()
        .chain(job.remainder.encode_utf16())
        .chain(job.following.encode_utf16()));
    SysFreeString(text.cast());
    matches
}

unsafe fn matching_tail(caret: &ComPtr, job: &Correction) -> Option<ComPtr> {
    let before = job.before.chars().count() as i32;
    let remainder = job.remainder.chars().count() as i32;
    let following = job.following.chars().count() as i32;
    let range = extend(caret, before, remainder + following)?;
    if !matches_text(&range, job)
        || extend(caret, before + remainder, following)
            .is_some_and(|at_end| matches_text(&at_end, job))
    {
        return None; // 已在末端或文字重複，以現有選取為準。
    }
    let raw = range.as_raw();
    let vt = ffi::vtable::<TextRange>(raw);
    let mut moved = 0;
    if following != 0
        && ((vt.move_by_unit)(raw, END, CHARACTER, -following, &mut moved) < 0
            || moved != -following)
    {
        return None;
    }
    ((vt.move_by_range)(raw, START, raw, END) >= 0).then_some(range)
}

#[cfg(test)]
mod tests {
    use super::suspected_boundary;

    #[test]
    fn chromium_boundary_counts_logical_breaks_and_keeps_utf16_intact() {
        assert_eq!(
            suspected_boundary("首行。\r\n下一行。\n第三行已經到了最後。"),
            Some(("第三行已經到了最".into(), "後。".into()))
        );
        assert!(suspected_boundary("一行").is_none());
        assert!(suspected_boundary("第一行\n第二行\n").is_none());
        assert!(suspected_boundary("第一行\n第二行中的表情符號🙂").is_none());
    }
}
