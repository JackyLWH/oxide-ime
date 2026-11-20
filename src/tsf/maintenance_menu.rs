//! TSF「中／英」按鈕的右鍵選單與維護程式啟動。
use crate::win32::{allow_foreground, hidden_command, wide, GetFocus};
use std::ptr;

use super::ffi::{
    self, ComPtr, ContextFullVtbl, ContextViewVtbl, HResult, Raw, ThreadManagerVtbl, UnknownVtbl,
};
use crate::{diagnostics, project_files};

struct Item {
    label: &'static str,
    action: &'static str,
}

const fn item(label: &'static str, action: &'static str) -> Option<Item> {
    Some(Item { label, action })
}

const ITEMS: &[Option<Item>] = &[
    item("開啟資料目錄", "OpenData"),
    item("開啟程式目錄", "OpenProgram"),
    None,
    item("檢查詞庫更新", "CheckUpdates"),
    item("定期更新設定…", "UpdateSettings"),
    #[cfg(feature = "private-discord")]
    item("輸入 Discord Webhook 網址…", "DiscordSettings"),
    None,
    item("重新啟動輸入法", "Restart"),
    item("指定程式重啟設定…", "RestartSettings"),
    None,
    item("詳細日誌", "Diagnostics"),
    item("匯出日誌…", "ExportLogs"),
    item("清空 Oxide IME 日誌…", "ClearLogs"),
    None,
    item("關於 Oxide IME", "About"),
];

const TF_LBMENUF_CHECKED: u32 = 0x0001;
const TF_LBMENUF_SEPARATOR: u32 = 0x0004;
const MF_CHECKED: u32 = 0x0008;
const MF_SEPARATOR: u32 = 0x0800;

#[repr(C)]
struct MenuVtbl {
    base: UnknownVtbl,
    add_menu_item:
        unsafe extern "system" fn(Raw, u32, u32, Raw, Raw, *const u16, u32, *mut Raw) -> HResult,
}

#[repr(C)]
struct DocumentManagerVtbl {
    base: UnknownVtbl,
    before_top: [usize; 3],
    get_top: unsafe extern "system" fn(Raw, *mut Raw) -> HResult,
}

#[link(name = "user32")]
extern "system" {
    fn CreatePopupMenu() -> Raw;
    fn AppendMenuW(menu: Raw, flags: u32, command: usize, label: *const u16) -> i32;
    fn DestroyMenu(menu: Raw) -> i32;
    fn TrackPopupMenuEx(menu: Raw, flags: u32, x: i32, y: i32, owner: Raw, params: Raw) -> u32;
    fn GetActiveWindow() -> Raw;
}

pub(super) unsafe fn initialize(menu: Raw) -> HResult {
    if menu.is_null() {
        return ffi::E_POINTER;
    }
    let add = ffi::vtable::<MenuVtbl>(menu).add_menu_item;
    let detailed = diagnostics::detail_channel_enabled();
    for (index, item) in ITEMS.iter().enumerate() {
        let (label, flags) = match item {
            Some(item) => (
                item.label,
                if detailed && item.action == "Diagnostics" {
                    TF_LBMENUF_CHECKED
                } else {
                    0
                },
            ),
            None => ("", TF_LBMENUF_SEPARATOR),
        };
        let label = wide(label);
        let hr = add(
            menu,
            index as u32 + 1,
            flags,
            ptr::null_mut(),
            ptr::null_mut(),
            label.as_ptr(),
            label.len() as u32 - 1,
            ptr::null_mut(),
        );
        if hr < 0 {
            return hr;
        }
    }
    ffi::S_OK
}

unsafe fn focused_context_window(thread_manager: Raw) -> Result<Raw, HResult> {
    let mut document = ptr::null_mut();
    let hr =
        (ffi::vtable::<ThreadManagerVtbl>(thread_manager).get_focus)(thread_manager, &mut document);
    let document = ComPtr::from_out(hr, document)?;
    let mut context = ptr::null_mut();
    let hr = (ffi::vtable::<DocumentManagerVtbl>(document.as_raw()).get_top)(
        document.as_raw(),
        &mut context,
    );
    let context = ComPtr::from_out(hr, context)?;
    let mut view = ptr::null_mut();
    let hr = (ffi::vtable::<ContextFullVtbl>(context.as_raw()).get_active_view)(
        context.as_raw(),
        &mut view,
    );
    let view = ComPtr::from_out(hr, view)?;
    let mut window = ptr::null_mut();
    let hr = (ffi::vtable::<ContextViewVtbl>(view.as_raw()).get_window)(view.as_raw(), &mut window);
    if hr >= 0 && !window.is_null() {
        Ok(window)
    } else {
        Err(hr)
    }
}

pub(super) unsafe fn popup(point: i64, thread_manager: Raw) -> HResult {
    let owner = focused_context_window(thread_manager).unwrap_or_else(|_| {
        let focus = GetFocus();
        if focus.is_null() {
            GetActiveWindow()
        } else {
            focus
        }
    });
    if owner.is_null() {
        return ffi::E_FAIL;
    }
    let menu = CreatePopupMenu();
    if menu.is_null() {
        return ffi::E_FAIL;
    }
    let detailed = diagnostics::detail_channel_enabled();
    for (index, item) in ITEMS.iter().enumerate() {
        let added = match item {
            Some(item) => {
                let flags = if detailed && item.action == "Diagnostics" {
                    MF_CHECKED
                } else {
                    0
                };
                AppendMenuW(menu, flags, index + 1, wide(item.label).as_ptr())
            }
            None => AppendMenuW(menu, MF_SEPARATOR, 0, ptr::null()),
        };
        if added == 0 {
            DestroyMenu(menu);
            return ffi::E_FAIL;
        }
    }
    const RETURN_COMMAND: u32 = 0x0100;
    const NO_NOTIFY: u32 = 0x0080;
    const BOTTOM_ALIGN: u32 = 0x0020;
    let command = TrackPopupMenuEx(
        menu,
        RETURN_COMMAND | NO_NOTIFY | BOTTOM_ALIGN,
        point as i32,
        (point >> 32) as i32,
        owner,
        ptr::null_mut(),
    );
    DestroyMenu(menu);
    if command == 0 {
        ffi::S_OK
    } else {
        select(command)
    }
}

pub(super) fn select(id: u32) -> HResult {
    let Some(item) = id
        .checked_sub(1)
        .and_then(|index| ITEMS.get(index as usize))
        .and_then(Option::as_ref)
    else {
        return ffi::E_INVALIDARG;
    };
    let result = launch(item);
    if let Err(error) = result {
        diagnostics::event(
            diagnostics::Level::Warning,
            "language_bar",
            "maintenance_start",
            &[
                ("command", i64::from(id)),
                ("win32", i64::from(error.raw_os_error().unwrap_or(0))),
            ],
        );
        return ffi::E_FAIL;
    }
    ffi::S_OK
}

fn launch(item: &Item) -> std::io::Result<()> {
    let root = project_files::dll_root().map_err(std::io::Error::other)?;
    let release = diagnostics::RELEASE.trim();
    let executable = project_files::maintenance(&root);
    let child = hidden_command(executable)
        .args([
            "--action",
            item.action,
            release,
            if cfg!(target_arch = "x86") {
                "x86"
            } else {
                "x64"
            },
        ])
        .spawn()?;
    // 使用者剛從輸入法選單啟動維護工具，交予該程序前景啟用權限。
    allow_foreground(child.id());
    Ok(())
}
