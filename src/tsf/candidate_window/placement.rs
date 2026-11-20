//! 以組字起點定位候選窗及狀態提示，並限制在目前螢幕的工作區。
use super::{Point, Raw, Rect};
use crate::win32::GetCurrentThreadId;
use std::ptr;

#[repr(C)]
struct GuiThreadInfo {
    size: u32,
    flags: u32,
    active: Raw,
    focus: Raw,
    capture: Raw,
    menu_owner: Raw,
    move_size: Raw,
    caret: Raw,
    caret_rect: Rect,
}

#[repr(C)]
struct MonitorInfo {
    size: u32,
    monitor: Rect,
    work: Rect,
    flags: u32,
}

#[link(name = "user32")]
extern "system" {
    fn GetGUIThreadInfo(thread: u32, info: *mut GuiThreadInfo) -> i32;
    fn ClientToScreen(window: Raw, point: *mut Point) -> i32;
    fn MonitorFromPoint(point: Point, flags: u32) -> Raw;
    fn GetMonitorInfoW(monitor: Raw, info: *mut MonitorInfo) -> i32;
}

pub(super) unsafe fn fallback_rect() -> Option<Rect> {
    let mut info = GuiThreadInfo {
        size: std::mem::size_of::<GuiThreadInfo>() as u32,
        flags: 0,
        active: ptr::null_mut(),
        focus: ptr::null_mut(),
        capture: ptr::null_mut(),
        menu_owner: ptr::null_mut(),
        move_size: ptr::null_mut(),
        caret: ptr::null_mut(),
        caret_rect: Rect::default(),
    };
    if GetGUIThreadInfo(GetCurrentThreadId(), &mut info) == 0 {
        return None;
    }
    if !info.caret.is_null() {
        let mut top = Point {
            x: info.caret_rect.left,
            y: info.caret_rect.top,
        };
        let mut bottom = Point {
            x: info.caret_rect.right,
            y: info.caret_rect.bottom,
        };
        if ClientToScreen(info.caret, &mut top) != 0 && ClientToScreen(info.caret, &mut bottom) != 0
        {
            return Some(Rect {
                left: top.x,
                top: top.y,
                right: bottom.x,
                bottom: bottom.y,
            });
        }
    }
    // 部分應用程式沒有傳回 TSF 文字座標，也不使用系統插入點。
    // 至少將候選窗放在取得焦點的控制項內，避免候選完全無法顯示。
    if info.focus.is_null() {
        return None;
    }
    let mut top = Point { x: 8, y: 8 };
    if ClientToScreen(info.focus, &mut top) == 0 {
        return None;
    }
    Some(Rect {
        left: top.x,
        top: top.y,
        right: top.x + 1,
        bottom: top.y + 24,
    })
}

fn fit_vertical(anchor: Rect, height: i32, gap: i32, work: Rect) -> i32 {
    let below = anchor.bottom.saturating_add(gap);
    let above = anchor.top.saturating_sub(height).saturating_sub(gap);
    let max_top = (work.bottom - height).max(work.top);
    if below >= work.top && below <= max_top {
        below
    } else if above >= work.top && above <= max_top {
        above
    } else {
        below.clamp(work.top, max_top)
    }
}

pub(super) unsafe fn monitor_work_area(anchor: Rect) -> Option<Rect> {
    work_area_at(Point {
        x: anchor.left,
        y: anchor.bottom,
    })
}

unsafe fn work_area_at(point: Point) -> Option<Rect> {
    let monitor = MonitorFromPoint(point, 2); // MONITOR_DEFAULTTONEAREST
    let mut info = MonitorInfo {
        size: std::mem::size_of::<MonitorInfo>() as u32,
        monitor: Rect::default(),
        work: Rect::default(),
        flags: 0,
    };
    (!monitor.is_null() && GetMonitorInfoW(monitor, &mut info) != 0).then_some(info.work)
}

pub(super) fn fit_to_monitor(
    anchor: Rect,
    width: i32,
    height: i32,
    gap: i32,
    work: Option<Rect>,
) -> (i32, i32) {
    let Some(work) = work else {
        return (anchor.left, anchor.bottom.saturating_add(gap));
    };
    let x = anchor
        .left
        .clamp(work.left, (work.right - width).max(work.left));
    (x, fit_vertical(anchor, height, gap, work))
}

pub(super) fn fit_toast(
    anchor: Rect,
    width: i32,
    height: i32,
    work: Option<Rect>,
    avoid: Option<Rect>,
) -> Option<(i32, i32)> {
    const GAP: i32 = 4;
    let x = anchor.right + GAP;
    let above = anchor.top - height - GAP;
    let y = if work.is_none_or(|work| above >= work.top) {
        above
    } else {
        anchor.bottom + GAP
    };
    let fit = |(x, y): (i32, i32)| {
        if let Some(work) = work {
            (
                x.clamp(work.left, (work.right - width).max(work.left)),
                y.clamp(work.top, (work.bottom - height).max(work.top)),
            )
        } else {
            (x, y)
        }
    };
    let preferred = fit((x, y));
    let Some(avoid) = avoid else {
        return Some(preferred);
    };
    [
        preferred,
        fit((avoid.right + GAP, y)),
        fit((avoid.left - width - GAP, y)),
        fit((x, avoid.bottom + GAP)),
        fit((x, avoid.top - height - GAP)),
    ]
    .into_iter()
    .find(|&(x, y)| {
        x + width <= avoid.left || x >= avoid.right || y + height <= avoid.top || y >= avoid.bottom
    })
}
