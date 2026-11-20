//! 組字只監聽宿主執行緒。F4 原生選單開啟時也接收桌面上的外部點擊。
use std::sync::atomic::Ordering;
use std::{cell::Cell, ptr};

use super::{
    input::{cancel_from_click, DISMISS_MENU},
    TextService,
};
use crate::tsf::ffi::Raw;
use crate::tsf::MODULE;
use crate::win32::GetCurrentThreadId;

const WH_GETMESSAGE: i32 = 3;
const WH_MOUSE_LL: i32 = 14;

#[repr(C)]
struct HookMessage {
    window: Raw,
    message: u32,
    wparam: usize,
    lparam: isize,
    time: u32,
    point_x: i32,
    point_y: i32,
    private: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct Point {
    x: i32,
    y: i32,
}

thread_local! {
    static MOUSE_HOOK_OWNER: Cell<*mut TextService> = const { Cell::new(ptr::null_mut()) };
}

#[link(name = "user32")]
extern "system" {
    fn SetWindowsHookExW(
        kind: i32,
        callback: Option<unsafe extern "system" fn(i32, usize, isize) -> isize>,
        module: Raw,
        thread: u32,
    ) -> Raw;
    fn UnhookWindowsHookEx(hook: Raw) -> i32;
    fn CallNextHookEx(hook: Raw, code: i32, wparam: usize, lparam: isize) -> isize;
    fn WindowFromPhysicalPoint(point: Point) -> Raw;
}

pub(super) unsafe fn install_mouse_hook(service: *mut TextService, menu: bool) {
    MOUSE_HOOK_OWNER.with(|owner| {
        let previous = owner.get();
        if !previous.is_null() && previous != service {
            remove_mouse_hook(previous);
        }
        if (*service).mouse_hook.is_null() {
            (*service).mouse_hook = SetWindowsHookExW(
                WH_GETMESSAGE,
                Some(mouse_message_hook),
                ptr::null_mut(),
                GetCurrentThreadId(),
            );
        }
        if menu && (*service).menu_mouse_hook.is_null() {
            // WH_MOUSE_LL 在本執行緒回呼，不會將 DLL 注入其他視窗的程序。
            (*service).menu_mouse_hook = SetWindowsHookExW(
                WH_MOUSE_LL,
                Some(desktop_mouse_hook),
                MODULE.load(Ordering::Relaxed),
                0,
            );
        } else if !menu {
            unhook(&mut (*service).menu_mouse_hook);
        }
        if !(*service).mouse_hook.is_null() || !(*service).menu_mouse_hook.is_null() {
            owner.set(service);
        }
    });
}

pub(super) unsafe fn remove_mouse_hook(service: *mut TextService) {
    MOUSE_HOOK_OWNER.with(|owner| {
        if owner.get() == service {
            owner.set(ptr::null_mut());
        }
    });
    unhook(&mut (*service).mouse_hook);
    unhook(&mut (*service).menu_mouse_hook);
}

unsafe fn unhook(handle: &mut Raw) {
    let hook = std::mem::replace(handle, ptr::null_mut());
    if !hook.is_null() {
        UnhookWindowsHookEx(hook);
    }
}

unsafe extern "system" fn desktop_mouse_hook(code: i32, wparam: usize, lparam: isize) -> isize {
    if code == 0 && lparam != 0 && matches!(wparam, 0x0201 | 0x0204 | 0x0207 | 0x020b) {
        MOUSE_HOOK_OWNER.with(|owner| {
            let service = owner.get();
            if service.is_null() || (*service).menu_mouse_hook.is_null() {
                return;
            }
            // MSLLHOOKSTRUCT 的第一欄是實體螢幕座標。只排入關閉訊息，讓原點擊照常送達。
            let window = WindowFromPhysicalPoint(*(lparam as *const Point));
            if !(*service).window.owns_window(window) {
                (*service)
                    .window
                    .post_event(DISMISS_MENU, (*service).menu_mouse_hook as isize);
            }
        });
    }
    CallNextHookEx(ptr::null_mut(), code, wparam, lparam)
}

unsafe extern "system" fn mouse_message_hook(code: i32, wparam: usize, lparam: isize) -> isize {
    if code == 0 && wparam == 1 && lparam != 0 {
        let message = &*(lparam as *const HookMessage);
        if matches!(
            message.message,
            0x0201
                | 0x0203
                | 0x0204
                | 0x0206
                | 0x0207
                | 0x0209
                | 0x020b
                | 0x020d
                | 0x00a1
                | 0x00a3
                | 0x00a4
                | 0x00a6
                | 0x00a7
                | 0x00a9
                | 0x00ab
                | 0x00ad
        ) {
            MOUSE_HOOK_OWNER.with(|owner| {
                let service = owner.get();
                if !service.is_null() && !(*service).window.owns_window(message.window) {
                    cancel_from_click(service);
                }
            });
        }
    }
    CallNextHookEx(ptr::null_mut(), code, wparam, lparam)
}
