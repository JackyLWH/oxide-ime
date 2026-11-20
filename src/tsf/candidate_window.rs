//! 不搶焦點的原生橫向候選窗。
use crate::win32::wide;
use crate::win32::{GetAncestor, GetFocus};
mod layout;
mod paint;
mod placement;
use layout::{CandidateFont, PANEL_FONT_POINTS};
use placement::{fit_toast, monitor_work_area};

use std::ptr;

use super::ffi::{Raw, Rect};
use super::gdi::{CreateFontW, DeleteObject};
use crate::engine::{CandidateView, SwitchView};
use paint::{paint_buffered, paint_layered};

const WS_POPUP: u32 = 0x80000000;
const SS_NOTIFY: u32 = 0x00000100;
const WS_EX_TOOLWINDOW: u32 = 0x00000080;
const WS_EX_TOPMOST: u32 = 0x00000008;
const WS_EX_NOACTIVATE: u32 = 0x08000000;
const WS_EX_LAYERED: u32 = 0x00080000;
const WM_SETTEXT: u32 = 0x000c;
const SW_HIDE: i32 = 0;
const SW_SHOWNOACTIVATE: i32 = 4;
const SWP_NOACTIVATE: u32 = 0x0010;
const SWP_NOSIZE: u32 = 0x0001;
const SWP_NOZORDER: u32 = 0x0004;
const LOGPIXELSY: i32 = 90;
const GWLP_WNDPROC: i32 = -4;
const GWLP_HWNDPARENT: i32 = -8;
const GWLP_USERDATA: i32 = -21;
const GWL_EXSTYLE: i32 = -20;
const WM_PAINT: u32 = 0x000f;
const WM_ERASEBKGND: u32 = 0x0014;
const WM_MOUSEACTIVATE: u32 = 0x0021;
const WM_TIMER: u32 = 0x0113;
const WM_LBUTTONDOWN: u32 = 0x0201;
const WM_OXIDE_EVENT: u32 = 0x8000;
const MA_NOACTIVATE: isize = 3;
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct Point {
    x: i32,
    y: i32,
}

#[repr(C)]
#[derive(Default)]
struct Size {
    cx: i32,
    cy: i32,
}

#[repr(C)]
struct PaintStruct {
    dc: Raw,
    erase: i32,
    paint: Rect,
    restore: i32,
    incremental: i32,
    reserved: [u8; 32],
}

struct PaintItem {
    runs: Vec<PaintRun>,
    rect: Rect,
    selected: bool,
}

struct PaintRun {
    text: Vec<u16>,
    font: Raw,
    width: i32,
}

struct PaintPage {
    text: Vec<u16>,
    rect: Rect,
}

struct PaintModel {
    original_proc: isize,
    font: Raw,
    page_font: Raw,
    items: Vec<PaintItem>,
    page: Option<PaintPage>,
    width: i32,
    height: i32,
    padding: i32,
    border_width: i32,
    corner_diameter: i32,
    layered: bool,
    clickable_count: usize,
    click_owner: Raw,
    click_handler: Option<unsafe fn(Raw, usize)>,
    event_handler: Option<unsafe fn(Raw, usize, isize)>,
    timer_owner: Raw,
    timer_handler: Option<unsafe fn(Raw)>,
    #[cfg(test)]
    show_events: std::cell::Cell<usize>,
    #[cfg(test)]
    hide_events: std::cell::Cell<usize>,
    #[cfg(test)]
    paint_events: std::cell::Cell<usize>,
}

#[link(name = "user32")]
extern "system" {
    fn CreateWindowExW(
        ex: u32,
        class: *const u16,
        name: *const u16,
        style: u32,
        x: i32,
        y: i32,
        width: i32,
        height: i32,
        parent: Raw,
        menu: Raw,
        instance: Raw,
        param: Raw,
    ) -> Raw;
    fn DestroyWindow(window: Raw) -> i32;
    fn ShowWindow(window: Raw, command: i32) -> i32;
    fn NotifyWinEvent(event: u32, window: Raw, object: i32, child: i32);
    fn IsWindowVisible(window: Raw) -> i32;
    fn GetWindowRect(window: Raw, rect: *mut Rect) -> i32;
    fn SetWindowTextW(window: Raw, text: *const u16) -> i32;
    fn SetWindowPos(window: Raw, after: Raw, x: i32, y: i32, cx: i32, cy: i32, flags: u32) -> i32;
    fn SetWindowRgn(window: Raw, region: Raw, redraw: i32) -> i32;
    fn GetDC(window: Raw) -> Raw;
    fn ReleaseDC(window: Raw, dc: Raw) -> i32;
    #[cfg_attr(target_arch = "x86", link_name = "GetWindowLongW")]
    fn GetWindowLongPtrW(window: Raw, index: i32) -> isize;
    #[cfg_attr(target_arch = "x86", link_name = "SetWindowLongW")]
    fn SetWindowLongPtrW(window: Raw, index: i32, value: isize) -> isize;
    fn CallWindowProcW(
        proc: isize,
        window: Raw,
        message: u32,
        wparam: usize,
        lparam: isize,
    ) -> isize;
    fn DefWindowProcW(window: Raw, message: u32, wparam: usize, lparam: isize) -> isize;
    fn BeginPaint(window: Raw, paint: *mut PaintStruct) -> Raw;
    fn EndPaint(window: Raw, paint: *const PaintStruct) -> i32;
    fn InvalidateRect(window: Raw, rect: *const Rect, erase: i32) -> i32;
    fn UpdateWindow(window: Raw) -> i32;
    fn PostMessageW(window: Raw, message: u32, wparam: usize, lparam: isize) -> i32;
    fn SetTimer(window: Raw, id: usize, interval: u32, callback: Raw) -> usize;
    fn KillTimer(window: Raw, id: usize) -> i32;
}

#[link(name = "gdi32")]
extern "system" {
    fn GetDeviceCaps(dc: Raw, index: i32) -> i32;
    fn CreateRoundRectRgn(
        left: i32,
        top: i32,
        right: i32,
        bottom: i32,
        width: i32,
        height: i32,
    ) -> Raw;
}

fn candidate_items(candidates: &[CandidateView]) -> Vec<(String, bool)> {
    candidates
        .iter()
        .enumerate()
        .map(|(i, candidate)| (format!("{}. {}", i + 1, candidate.text), candidate.selected))
        .collect()
}

fn candidate_label(items: &[(String, bool)], page: usize) -> String {
    let page = format!("第 {} 頁", page + 1);
    items
        .iter()
        .map(|(text, _)| text.as_str())
        .chain(std::iter::once(page.as_str()))
        .collect::<Vec<_>>()
        .join("   ")
}

unsafe fn hide_window(window: Raw) {
    KillTimer(window, 1);
    hide_visible_window(window);
}

unsafe fn hide_visible_window(window: Raw) {
    if IsWindowVisible(window) != 0 {
        ShowWindow(window, SW_HIDE);
        NotifyWinEvent(0x8028, window, -4, 0);
    }
}

unsafe extern "system" fn window_proc(
    window: Raw,
    message: u32,
    wparam: usize,
    lparam: isize,
) -> isize {
    let model = GetWindowLongPtrW(window, GWLP_USERDATA) as *const PaintModel;
    if !model.is_null() {
        if message == WM_MOUSEACTIVATE {
            // STATIC 的預設程序可能回覆 MA_ACTIVATE，令宿主在點擊前終止組字。
            return MA_NOACTIVATE;
        }
        #[cfg(test)]
        if message == 0x0018 {
            let events = if wparam == 0 {
                &(*model).hide_events
            } else {
                &(*model).show_events
            };
            events.set(events.get() + 1);
        }
        if message == WM_LBUTTONDOWN {
            let x = (lparam as u16 as i16) as i32;
            let y = ((lparam >> 16) as u16 as i16) as i32;
            let index = (*model)
                .items
                .iter()
                .take((*model).clickable_count)
                .position(|item| {
                    x >= item.rect.left
                        && x < item.rect.right
                        && y >= item.rect.top
                        && y < item.rect.bottom
                });
            if let (Some(index), Some(handler)) = (index, (*model).click_handler) {
                let owner = (*model).click_owner;
                if !owner.is_null() {
                    handler(owner, index);
                }
            }
            return 0;
        }
        if message == WM_OXIDE_EVENT {
            if let Some(handler) = (*model).event_handler {
                handler((*model).click_owner, wparam, lparam);
            }
            return 0;
        }
        if message == WM_TIMER && wparam == 1 {
            hide_window(window);
            if let Some(handler) = (*model).timer_handler {
                let owner = (*model).timer_owner;
                if !owner.is_null() {
                    handler(owner);
                }
            }
            return 0;
        }
        if message == WM_ERASEBKGND {
            return 1;
        }
        if message == WM_SETTEXT {
            // 只更新視窗的無障礙名稱；STATIC 的預設繪字交由下方的雙緩衝重繪。
            return DefWindowProcW(window, message, wparam, lparam);
        }
        if message == WM_PAINT {
            let mut paint: PaintStruct = std::mem::zeroed();
            let dc = BeginPaint(window, &mut paint);
            if !dc.is_null() && !(*model).layered {
                paint_buffered(dc, &*model);
                #[cfg(test)]
                (*model).paint_events.set((*model).paint_events.get() + 1);
            }
            EndPaint(window, &paint);
            return 0;
        }
        return CallWindowProcW((*model).original_proc, window, message, wparam, lparam);
    }
    DefWindowProcW(window, message, wparam, lparam)
}

pub(crate) struct CandidateWindow {
    window: Raw,
    styled_fonts: Option<(i32, Vec<CandidateFont>)>,
    anchor: Option<Rect>,
    rounded: bool,
    model: Box<PaintModel>,
}

#[derive(Clone, Copy)]
enum WindowContent {
    Candidates(usize),
    Switches,
    Mode(Option<Rect>),
}

impl CandidateWindow {
    pub(crate) fn new() -> Self {
        Self {
            window: ptr::null_mut(),
            styled_fonts: None,
            anchor: None,
            rounded: false,
            model: Box::new(PaintModel {
                original_proc: 0,
                font: ptr::null_mut(),
                page_font: ptr::null_mut(),
                items: Vec::new(),
                page: None,
                width: 0,
                height: 0,
                padding: 2,
                border_width: 1,
                corner_diameter: 0,
                layered: true,
                clickable_count: 0,
                click_owner: ptr::null_mut(),
                click_handler: None,
                event_handler: None,
                timer_owner: ptr::null_mut(),
                timer_handler: None,
                #[cfg(test)]
                show_events: std::cell::Cell::new(0),
                #[cfg(test)]
                hide_events: std::cell::Cell::new(0),
                #[cfg(test)]
                paint_events: std::cell::Cell::new(0),
            }),
        }
    }

    pub(crate) fn hide(&mut self) {
        if self.has_window() {
            unsafe { hide_window(self.window) };
        }
    }

    pub(crate) fn set_click_handler(&mut self, owner: Raw, handler: unsafe fn(Raw, usize)) {
        self.model.click_owner = owner;
        self.model.click_handler = Some(handler);
    }

    pub(crate) fn owns_window(&self, window: Raw) -> bool {
        !window.is_null() && self.window == window
    }

    pub(crate) fn set_event_handler(&mut self, handler: unsafe fn(Raw, usize, isize)) {
        self.model.event_handler = Some(handler);
    }

    pub(crate) fn post_event(&self, kind: usize, token: isize) -> bool {
        self.has_window() && unsafe { PostMessageW(self.window, WM_OXIDE_EVENT, kind, token) != 0 }
    }

    pub(crate) fn visible_rect(&self) -> Option<Rect> {
        let mut rect = Rect::default();
        (self.has_window()
            && unsafe {
                IsWindowVisible(self.window) != 0 && GetWindowRect(self.window, &mut rect) != 0
            })
        .then_some(rect)
    }

    fn has_window(&self) -> bool {
        !self.window.is_null()
            && unsafe { GetWindowLongPtrW(self.window, GWLP_USERDATA) }
                == (&*self.model as *const PaintModel) as isize
    }

    pub(crate) fn set_timer_handler(&mut self, owner: Raw, handler: unsafe fn(Raw)) {
        self.model.timer_owner = owner;
        self.model.timer_handler = Some(handler);
    }

    pub(crate) fn start_mode_timer(&mut self) {
        unsafe {
            if self.ensure_window() {
                KillTimer(self.window, 1);
                SetTimer(self.window, 1, 1200, ptr::null_mut());
            }
        }
    }

    pub(crate) fn reset(&mut self) {
        self.hide();
        self.anchor = None;
    }

    pub(crate) fn update(
        &mut self,
        candidates: &[CandidateView],
        page: usize,
        anchor: Option<Rect>,
    ) {
        let items = candidate_items(candidates);
        self.update_items(
            &items,
            WindowContent::Candidates(page),
            &candidate_label(&items, page),
            anchor,
        );
    }

    pub(crate) fn update_switches(&mut self, switches: &[SwitchView], anchor: Option<Rect>) {
        let items: Vec<(String, bool)> = switches
            .iter()
            .enumerate()
            .map(|(i, option)| {
                (
                    format!("{}. {}　{}", i + 1, option.name, option.state),
                    option.selected,
                )
            })
            .collect();
        let accessible = items
            .iter()
            .map(|(s, _)| s.as_str())
            .collect::<Vec<_>>()
            .join("  ");
        self.update_items(&items, WindowContent::Switches, &accessible, anchor);
    }

    pub(crate) fn show_toast(&mut self, label: &str, anchor: Option<Rect>, avoid: Option<Rect>) {
        self.anchor = None;
        self.update_items(
            &[(label.to_owned(), true)],
            WindowContent::Mode(avoid),
            label,
            anchor,
        );
        self.start_mode_timer();
    }

    pub(crate) fn reposition_toast(&self, avoid: Option<Rect>) {
        let (Some(rect), Some(anchor)) = (self.visible_rect(), self.anchor) else {
            return;
        };
        unsafe {
            let work = monitor_work_area(anchor);
            match fit_toast(anchor, self.model.width, self.model.height, work, avoid) {
                Some((x, y)) if (x, y) != (rect.left, rect.top) => {
                    SetWindowPos(
                        self.window,
                        ptr::null_mut(),
                        x,
                        y,
                        0,
                        0,
                        SWP_NOACTIVATE | SWP_NOSIZE | SWP_NOZORDER,
                    );
                    NotifyWinEvent(0x8029, self.window, -4, 0);
                }
                None => hide_visible_window(self.window),
                _ => {}
            }
        }
    }

    fn update_items(
        &mut self,
        items: &[(String, bool)],
        content: WindowContent,
        accessible: &str,
        anchor: Option<Rect>,
    ) {
        if self.anchor.is_none() {
            self.anchor =
                anchor.filter(|r| r.left != 0 || r.top != 0 || r.right != 0 || r.bottom != 0);
        }
        if items.is_empty() {
            self.hide();
            return;
        }
        unsafe {
            if !self.ensure_window() {
                return;
            }
            let focus = GetFocus();
            if !focus.is_null() {
                let ancestor = GetAncestor(focus, 2); // GA_ROOT
                let owner = if ancestor.is_null() { focus } else { ancestor };
                if GetWindowLongPtrW(self.window, GWLP_HWNDPARENT) != owner as isize {
                    SetWindowLongPtrW(self.window, GWLP_HWNDPARENT, owner as isize);
                }
            }
            let dc = GetDC(self.window);
            if dc.is_null() {
                return;
            }
            let Some((x, y)) = self.layout(items, content, dc) else {
                self.hide();
                return;
            };
            let (width, height) = (self.model.width, self.model.height);
            let mut current = Rect::default();
            let exists = GetWindowRect(self.window, &mut current) != 0;
            let resized = !exists
                || current.right - current.left != width
                || current.bottom - current.top != height;
            let moved = !exists || current.left != x || current.top != y || resized;
            if self.model.layered {
                if paint_layered(self.window, &self.model, x, y) {
                    #[cfg(test)]
                    self.model
                        .paint_events
                        .set(self.model.paint_events.get() + 1);
                } else {
                    // 分層視窗不可用時退回原本的 GDI 繪圖，避免候選框消失。
                    let style = GetWindowLongPtrW(self.window, GWL_EXSTYLE);
                    SetWindowLongPtrW(self.window, GWL_EXSTYLE, style & !(WS_EX_LAYERED as isize));
                    self.model.layered = false;
                }
            }
            if !self.model.layered && moved {
                SetWindowPos(
                    self.window,
                    (-1isize) as Raw,
                    x,
                    y,
                    width,
                    height,
                    SWP_NOACTIVATE,
                );
            }
            if moved && IsWindowVisible(self.window) != 0 {
                NotifyWinEvent(0x8029, self.window, -4, 0);
            }
            if !self.model.layered && (resized || !self.rounded) {
                let region = CreateRoundRectRgn(
                    0,
                    0,
                    width + 1,
                    height + 1,
                    self.model.corner_diameter,
                    self.model.corner_diameter,
                );
                if !region.is_null() {
                    if SetWindowRgn(self.window, region, 0) != 0 {
                        self.rounded = true;
                    } else {
                        DeleteObject(region);
                    }
                }
            }
            let text = wide(accessible);
            let _ = SetWindowTextW(self.window, text.as_ptr());
            if !self.model.layered {
                InvalidateRect(self.window, ptr::null(), 0);
            }
            if IsWindowVisible(self.window) == 0 {
                ShowWindow(self.window, SW_SHOWNOACTIVATE);
                NotifyWinEvent(0x8027, self.window, -4, 0);
            }
            // 新顯示的視窗仍要處理 Windows 自行安排的 WM_PAINT。
            UpdateWindow(self.window);
        }
    }

    pub(crate) unsafe fn ensure_window(&mut self) -> bool {
        if self.has_window() {
            return true;
        }
        if !self.window.is_null() {
            // Windows 會隨擁有者一併銷毀彈窗，宿主重建控制項後需重新建立。
            crate::diagnostics::event(crate::diagnostics::Level::Detail, "window", "recreate", &[]);
        }
        self.rounded = false;
        self.model.layered = true;
        self.window = CreateWindowExW(
            WS_EX_TOOLWINDOW | WS_EX_TOPMOST | WS_EX_NOACTIVATE | WS_EX_LAYERED,
            wide("STATIC").as_ptr(),
            wide("").as_ptr(),
            // STATIC 預設將滑鼠命中視為透明，必須開啟 SS_NOTIFY 才會收到實際點擊。
            WS_POPUP | SS_NOTIFY,
            0,
            0,
            0,
            0,
            // 沉浸式程式必須在建立視窗時指定擁有者，之後再設定可能被宿主遮蓋。
            GetFocus(),
            ptr::null_mut(),
            ptr::null_mut(),
            ptr::null_mut(),
        );
        if self.window.is_null() {
            static FAILED: crate::diagnostics::WarningLimit =
                crate::diagnostics::WarningLimit::new();
            let error = std::io::Error::last_os_error().raw_os_error().unwrap_or(0);
            if FAILED.ready() {
                crate::diagnostics::event(
                    crate::diagnostics::Level::Warning,
                    "window",
                    "create_failed",
                    &[("win32", i64::from(error))],
                );
            }
            return false;
        }
        let model = (&mut *self.model as *mut PaintModel) as isize;
        SetWindowLongPtrW(self.window, GWLP_USERDATA, model);
        self.model.original_proc =
            SetWindowLongPtrW(self.window, GWLP_WNDPROC, window_proc as *const () as isize);
        if self.model.original_proc == 0 || GetWindowLongPtrW(self.window, GWLP_USERDATA) != model {
            crate::diagnostics::event(
                crate::diagnostics::Level::Error,
                "window",
                "subclass_failed",
                &[],
            );
            DestroyWindow(self.window);
            self.window = ptr::null_mut();
            return false;
        }
        let dc = GetDC(self.window);
        let dpi = if dc.is_null() {
            96
        } else {
            GetDeviceCaps(dc, LOGPIXELSY)
        };
        if !dc.is_null() {
            ReleaseDC(self.window, dc);
        }
        if !self.model.font.is_null() {
            DeleteObject(self.model.font);
        }
        if !self.model.page_font.is_null() {
            DeleteObject(self.model.page_font);
        }
        let height = -((PANEL_FONT_POINTS * dpi.max(1) + 36) / 72);
        self.model.font = CreateFontW(
            height,
            0,
            0,
            0,
            400,
            0,
            0,
            0,
            1,
            0,
            0,
            0,
            0,
            wide("DFKai-SB").as_ptr(),
        );
        let page_height = -((9 * dpi.max(1) + 36) / 72);
        self.model.page_font = CreateFontW(
            page_height,
            0,
            0,
            0,
            400,
            0,
            0,
            0,
            1,
            0,
            0,
            0,
            0,
            wide("Segoe UI").as_ptr(),
        );
        true
    }
}

impl Drop for CandidateWindow {
    fn drop(&mut self) {
        unsafe {
            if self.has_window() {
                DestroyWindow(self.window);
            }
            if !self.model.font.is_null() {
                DeleteObject(self.model.font);
            }
            if !self.model.page_font.is_null() {
                DeleteObject(self.model.page_font);
            }
        }
    }
}

#[cfg(test)]
mod tests;
