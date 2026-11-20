//! Windows 輸入指示器的原生中英文模式圖示。
mod mode_notification;

use crate::broker::{self, ModeAction};
use crate::diagnostics::{self, Level};
use crate::win32::{registry, wide};
use std::mem::offset_of;
use std::ptr;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use super::ffi::{
    self, CompartmentSinkVtbl, Guid, HResult, LangBarButtonVtbl, LangBarInfo, LangBarMgrVtbl,
    LangBarSinkVtbl, Raw, Rect, SourceVtbl, UnknownVtbl,
};
use super::gdi::{
    CreateFontW, DeleteObject, Dib, DrawTextW, GdiFlush, SelectObject, SetBkMode, SetTextColor,
    Surface,
};
use super::{CLSID, OBJECTS};
use mode_notification::{ModeNotification, MODE_CHANGED};

const MODE_FACE: &str = "Microsoft YaHei";
const CONNECT_E_ADVISELIMIT: HResult = 0x80040201u32 as i32;
const CONNECT_E_NOCONNECTION: HResult = 0x80040200u32 as i32;
const CONNECT_E_CANNOTCONNECT: HResult = 0x80040202u32 as i32;

pub(super) fn mode_label(ascii: bool) -> &'static str {
    if ascii {
        "英"
    } else {
        "中"
    }
}

#[repr(C)]
struct Button {
    button_vtbl: *const LangBarButtonVtbl,
    source_vtbl: *const SourceVtbl,
    mode_vtbl: *const CompartmentSinkVtbl,
    refs: AtomicU32,
    ascii: AtomicBool,
    owner: Raw,
    thread_manager: Raw,
    toggle: unsafe fn(Raw),
    sink: Raw,
}

static BUTTON_VTBL: LangBarButtonVtbl = LangBarButtonVtbl {
    base: UnknownVtbl {
        query,
        add_ref,
        release,
    },
    get_info,
    get_status,
    show,
    get_tooltip,
    on_click,
    init_menu,
    menu_select,
    get_icon,
    get_text,
};
static SOURCE_VTBL: SourceVtbl = SourceVtbl {
    base: UnknownVtbl {
        query: source_query,
        add_ref: source_add_ref,
        release: source_release,
    },
    advise,
    unadvise,
};
static MODE_VTBL: CompartmentSinkVtbl = CompartmentSinkVtbl {
    base: UnknownVtbl {
        query: mode_query,
        add_ref: mode_add_ref,
        release: mode_release,
    },
    on_change: mode_changed,
};

pub(crate) struct LanguageBar {
    manager: Raw,
    button: Raw,
    notification: Option<ModeNotification>,
}

impl LanguageBar {
    pub(crate) fn new() -> Self {
        Self {
            manager: ptr::null_mut(),
            button: ptr::null_mut(),
            notification: None,
        }
    }

    pub(crate) unsafe fn attach(
        &mut self,
        thread_manager: Raw,
        owner: Raw,
        toggle: unsafe fn(Raw),
        ascii: bool,
    ) {
        if !self.button.is_null() {
            return;
        }
        self.notification = ModeNotification::open(thread_manager).ok();
        let manager = match ffi::query(thread_manager, &ffi::IID_ITfLangBarItemMgr) {
            Ok(manager) => manager,
            Err(hr) => {
                crate::diagnostics::event(
                    crate::diagnostics::Level::Detail,
                    "language_bar",
                    "manager_unavailable",
                    &[("hresult", crate::diagnostics::hresult(hr))],
                );
                return;
            }
        };
        let button = Box::into_raw(Box::new(Button {
            button_vtbl: &BUTTON_VTBL,
            source_vtbl: &SOURCE_VTBL,
            mode_vtbl: &MODE_VTBL,
            refs: AtomicU32::new(1),
            ascii: AtomicBool::new(ascii),
            owner,
            thread_manager,
            toggle,
            sink: ptr::null_mut(),
        }));
        OBJECTS.fetch_add(1, Ordering::SeqCst);
        let hr = (ffi::vtable::<LangBarMgrVtbl>(manager).add_item)(manager, button.cast());
        if hr < 0 {
            crate::diagnostics::result("language_bar", "add_item", hr);
            release(button.cast());
            ffi::release(manager);
            return;
        }
        self.manager = manager;
        self.button = button.cast();
        if let Some(notification) = self.notification.as_mut() {
            let sink = ptr::addr_of_mut!((*button).mode_vtbl).cast();
            if let Err(hr) = notification.subscribe(sink) {
                diagnostics::event(
                    Level::Detail,
                    "language_bar",
                    "mode_subscribe",
                    &[("hresult", diagnostics::hresult(hr))],
                );
            }
        }
    }

    pub(crate) unsafe fn set_ascii(&mut self, ascii: bool) {
        if !self.button.is_null() {
            update_ascii(self.button.cast(), ascii);
        }
    }

    pub(crate) unsafe fn broadcast_ascii(&self, client_id: u32, ascii: bool) {
        if let Some(notification) = self.notification.as_ref() {
            notification.publish(client_id, ascii);
        }
    }

    pub(crate) unsafe fn detach(&mut self) {
        let button = std::mem::replace(&mut self.button, ptr::null_mut());
        let manager = std::mem::replace(&mut self.manager, ptr::null_mut());
        if !button.is_null() {
            (*(button as *mut Button)).owner = ptr::null_mut();
        }
        drop(self.notification.take());
        if !button.is_null() {
            if !manager.is_null() {
                let _ = (ffi::vtable::<LangBarMgrVtbl>(manager).remove_item)(manager, button);
            }
            release(button);
        }
        if !manager.is_null() {
            ffi::release(manager);
        }
    }
}

unsafe fn update_ascii(button: *mut Button, ascii: bool) -> bool {
    if (*button).ascii.swap(ascii, Ordering::SeqCst) == ascii {
        return false;
    }
    let sink = (*button).sink;
    if !sink.is_null() {
        ffi::add_ref(sink);
        let _ = (ffi::vtable::<LangBarSinkVtbl>(sink).on_update)(sink, 0x1 | 0x2 | 0x4);
        ffi::release(sink);
    }
    true
}

unsafe fn displayed_ascii(button: *mut Button) -> bool {
    let cached = (*button).ascii.load(Ordering::SeqCst);
    if (*button).owner.is_null() {
        return cached;
    }
    // Windows 可隨時向仍在背景的程序取圖標。從同一來源取得目前的模式。
    broker::modes(ModeAction::Read).map_or(cached, |modes| modes.ascii)
}

impl Drop for LanguageBar {
    fn drop(&mut self) {
        unsafe {
            self.detach();
        }
    }
}

unsafe fn source_owner(this: Raw) -> Raw {
    (this as *mut u8)
        .sub(offset_of!(Button, source_vtbl))
        .cast()
}
unsafe fn mode_owner(this: Raw) -> Raw {
    (this as *mut u8).sub(offset_of!(Button, mode_vtbl)).cast()
}
unsafe extern "system" fn query(this: Raw, iid: *const Guid, out: *mut Raw) -> HResult {
    if out.is_null() {
        return ffi::E_POINTER;
    }
    *out = ptr::null_mut();
    if iid.is_null() {
        return ffi::E_POINTER;
    }
    if *iid == ffi::IID_IUnknown
        || *iid == ffi::IID_ITfLangBarItem
        || *iid == ffi::IID_ITfLangBarItemButton
    {
        *out = this;
    } else if *iid == ffi::IID_ITfSource {
        *out = ptr::addr_of_mut!((*(this as *mut Button)).source_vtbl).cast();
    } else if *iid == ffi::IID_ITfCompartmentEventSink {
        *out = ptr::addr_of_mut!((*(this as *mut Button)).mode_vtbl).cast();
    } else {
        return ffi::E_NOINTERFACE;
    }
    add_ref(this);
    ffi::S_OK
}
unsafe extern "system" fn add_ref(this: Raw) -> u32 {
    (*(this as *mut Button))
        .refs
        .fetch_add(1, Ordering::Relaxed)
        + 1
}
unsafe extern "system" fn release(this: Raw) -> u32 {
    let button = this as *mut Button;
    let refs = (*button).refs.fetch_sub(1, Ordering::Release) - 1;
    if refs == 0 {
        std::sync::atomic::fence(Ordering::Acquire);
        if !(*button).sink.is_null() {
            ffi::release((*button).sink);
        }
        drop(Box::from_raw(button));
        OBJECTS.fetch_sub(1, Ordering::SeqCst);
    }
    refs
}
unsafe extern "system" fn source_query(this: Raw, iid: *const Guid, out: *mut Raw) -> HResult {
    query(source_owner(this), iid, out)
}
unsafe extern "system" fn source_add_ref(this: Raw) -> u32 {
    add_ref(source_owner(this))
}
unsafe extern "system" fn source_release(this: Raw) -> u32 {
    release(source_owner(this))
}
unsafe extern "system" fn mode_query(this: Raw, iid: *const Guid, out: *mut Raw) -> HResult {
    query(mode_owner(this), iid, out)
}
unsafe extern "system" fn mode_add_ref(this: Raw) -> u32 {
    add_ref(mode_owner(this))
}
unsafe extern "system" fn mode_release(this: Raw) -> u32 {
    release(mode_owner(this))
}
unsafe extern "system" fn mode_changed(this: Raw, guid: *const Guid) -> HResult {
    let button = mode_owner(this) as *mut Button;
    if !guid.is_null() && *guid == MODE_CHANGED && !(*button).owner.is_null() {
        if let Ok(modes) = broker::modes(ModeAction::Read) {
            if update_ascii(button, modes.ascii) {
                diagnostics::event(
                    Level::Detail,
                    "language_bar",
                    "mode_refresh",
                    &[("ascii_mode", i64::from(modes.ascii))],
                );
            }
        }
    }
    ffi::S_OK
}

unsafe extern "system" fn get_info(_this: Raw, info: *mut LangBarInfo) -> HResult {
    if info.is_null() {
        return ffi::E_POINTER;
    }
    let mut description = [0u16; 32];
    for (slot, ch) in description
        .iter_mut()
        .zip("Oxide IME 中英文".encode_utf16())
    {
        *slot = ch;
    }
    *info = LangBarInfo {
        service: CLSID,
        item: ffi::GUID_LBI_INPUTMODE,
        style: 0x0001_0000,
        sort: 0,
        description,
    };
    ffi::S_OK
}
unsafe extern "system" fn get_status(_this: Raw, status: *mut u32) -> HResult {
    if status.is_null() {
        return ffi::E_POINTER;
    }
    *status = 0;
    ffi::S_OK
}
unsafe extern "system" fn show(_this: Raw, _show: i32) -> HResult {
    ffi::S_OK
}
unsafe extern "system" fn on_click(
    this: Raw,
    click: i32,
    point: i64,
    _rect: *const Rect,
) -> HResult {
    let button = this as *mut Button;
    if (*button).owner.is_null() {
        return ffi::S_OK;
    }
    match click {
        1 => super::maintenance_menu::popup(point, (*button).thread_manager), // TF_LBI_CLK_RIGHT
        2 => {
            ((*button).toggle)((*button).owner);
            ffi::S_OK
        }
        _ => ffi::S_OK,
    }
}
unsafe extern "system" fn init_menu(_this: Raw, menu: Raw) -> HResult {
    super::maintenance_menu::initialize(menu)
}
unsafe extern "system" fn menu_select(_this: Raw, id: u32) -> HResult {
    super::maintenance_menu::select(id)
}
unsafe extern "system" fn get_tooltip(_this: Raw, result: *mut Raw) -> HResult {
    ffi::write_bstr("Oxide IME　左鍵切換中英，右鍵開啟選單", result)
}
unsafe extern "system" fn get_icon(this: Raw, icon: *mut Raw) -> HResult {
    if icon.is_null() {
        return ffi::E_POINTER;
    }
    *icon = make_icon(displayed_ascii(this.cast()));
    if (*icon).is_null() {
        ffi::E_FAIL
    } else {
        ffi::S_OK
    }
}
unsafe extern "system" fn get_text(this: Raw, result: *mut Raw) -> HResult {
    let ascii = displayed_ascii(this.cast());
    ffi::write_bstr(mode_label(ascii), result)
}
unsafe extern "system" fn advise(
    this: Raw,
    iid: *const Guid,
    sink: Raw,
    cookie: *mut u32,
) -> HResult {
    if iid.is_null() || cookie.is_null() || sink.is_null() {
        return ffi::E_POINTER;
    }
    if *iid != ffi::IID_ITfLangBarItemSink {
        return CONNECT_E_CANNOTCONNECT;
    }
    let button = source_owner(this) as *mut Button;
    if !(*button).sink.is_null() {
        return CONNECT_E_ADVISELIMIT;
    }
    let sink = match ffi::query(sink, &ffi::IID_ITfLangBarItemSink) {
        Ok(sink) => sink,
        Err(_) => return CONNECT_E_CANNOTCONNECT,
    };
    (*button).sink = sink;
    *cookie = 1;
    ffi::S_OK
}
unsafe extern "system" fn unadvise(this: Raw, cookie: u32) -> HResult {
    let button = source_owner(this) as *mut Button;
    if cookie != 1 || (*button).sink.is_null() {
        return CONNECT_E_NOCONNECTION;
    }
    let sink = std::mem::replace(&mut (*button).sink, ptr::null_mut());
    ffi::release(sink);
    ffi::S_OK
}

#[repr(C)]
struct IconInfo {
    is_icon: i32,
    hotspot_x: u32,
    hotspot_y: u32,
    mask: Raw,
    color: Raw,
}

#[link(name = "gdi32")]
extern "system" {
    fn CreateBitmap(width: i32, height: i32, planes: u32, bits: u32, pixels: *const u8) -> Raw;
}
#[link(name = "user32")]
extern "system" {
    fn CreateIconIndirect(info: *const IconInfo) -> Raw;
    fn GetSysColor(index: i32) -> u32;
}
unsafe fn light_taskbar() -> bool {
    let path = wide(r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize");
    if let Some(light) = registry::read_dword(registry::HKCU, path.as_ptr(), "SystemUsesLightTheme")
    {
        return light != 0;
    }
    let text = GetSysColor(8); // COLOR_WINDOWTEXT：讀取設定失敗時採用系統文字對比。
    (text & 255) + ((text >> 8) & 255) + ((text >> 16) & 255) < 3 * 128
}

unsafe fn make_icon(ascii: bool) -> Raw {
    let Some(pixels) = icon_pixels(ascii, light_taskbar()) else {
        return ptr::null_mut();
    };
    icon_from_pixels(&pixels)
}

unsafe fn icon_from_pixels(pixels: &[u32; 256]) -> Raw {
    let Some(mut color) = Dib::new(ptr::null_mut(), 16, 16) else {
        return ptr::null_mut();
    };
    color.pixels_mut().copy_from_slice(pixels);
    let mut mask = [0u8; 32];
    for y in 0..16 {
        for x in 0..16 {
            if pixels[y * 16 + x] >> 24 == 0 {
                mask[y * 2 + x / 8] |= 0x80 >> (x % 8);
            }
        }
    }
    let mask = CreateBitmap(16, 16, 1, 1, mask.as_ptr());
    let icon = if mask.is_null() {
        ptr::null_mut()
    } else {
        CreateIconIndirect(&IconInfo {
            is_icon: 1,
            hotspot_x: 0,
            hotspot_y: 0,
            mask,
            color: color.handle,
        })
    };
    if !mask.is_null() {
        DeleteObject(mask);
    }
    icon
}

unsafe fn icon_pixels(ascii: bool, light: bool) -> Option<[u32; 256]> {
    let surface = Surface::new(16, 16)?;
    let dc = surface.dc;
    let font = CreateFontW(
        -14,
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
        4,
        0,
        wide(MODE_FACE).as_ptr(),
    );
    if font.is_null() {
        return None;
    }
    let old_font = SelectObject(dc, font);
    if old_font.is_null() || old_font as isize == -1 {
        DeleteObject(font);
        return None;
    }
    SetTextColor(dc, 0x00ff_ffff);
    SetBkMode(dc, 1);
    let label: Vec<u16> = mode_label(ascii).encode_utf16().collect();
    let mut rect = Rect {
        left: 0,
        top: 0,
        right: 16,
        bottom: 16,
    };
    DrawTextW(
        dc,
        label.as_ptr(),
        label.len() as i32,
        &mut rect,
        0x1 | 0x4 | 0x20 | 0x800,
    );
    GdiFlush();
    let pixels = surface.bitmap.pixels();
    let mut result = [0u32; 256];
    for (out, pixel) in result.iter_mut().zip(pixels.iter().copied()) {
        let alpha = pixel & 0xff;
        // HICON 的色彩須預乘 alpha；淺色工作列畫黑字，深色則畫白字。
        *out = (alpha << 24) | if light { 0 } else { alpha * 0x0001_0101 };
    }
    SelectObject(dc, old_font);
    DeleteObject(font);
    Some(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[repr(C)]
    struct Sink {
        vtbl: *const LangBarSinkVtbl,
        refs: AtomicU32,
        calls: AtomicU32,
        flags: AtomicU32,
    }
    static SINK_VTBL: LangBarSinkVtbl = LangBarSinkVtbl {
        base: UnknownVtbl {
            query: sink_query,
            add_ref: sink_add_ref,
            release: sink_release,
        },
        on_update: sink_update,
    };
    unsafe extern "system" fn sink_query(this: Raw, iid: *const Guid, out: *mut Raw) -> HResult {
        if out.is_null() || iid.is_null() {
            return ffi::E_POINTER;
        }
        *out = ptr::null_mut();
        if *iid != ffi::IID_IUnknown && *iid != ffi::IID_ITfLangBarItemSink {
            return ffi::E_NOINTERFACE;
        }
        *out = this;
        sink_add_ref(this);
        ffi::S_OK
    }
    unsafe extern "system" fn sink_add_ref(this: Raw) -> u32 {
        (*(this as *mut Sink)).refs.fetch_add(1, Ordering::SeqCst) + 1
    }
    unsafe extern "system" fn sink_release(this: Raw) -> u32 {
        (*(this as *mut Sink)).refs.fetch_sub(1, Ordering::SeqCst) - 1
    }
    unsafe extern "system" fn sink_update(this: Raw, flags: u32) -> HResult {
        let sink = &*(this as *mut Sink);
        sink.calls.fetch_add(1, Ordering::SeqCst);
        sink.flags.store(flags, Ordering::SeqCst);
        ffi::S_OK
    }
    #[link(name = "user32")]
    extern "system" {
        fn DestroyIcon(icon: Raw) -> i32;
        fn DrawIconEx(
            dc: Raw,
            x: i32,
            y: i32,
            icon: Raw,
            width: i32,
            height: i32,
            step: u32,
            brush: Raw,
            flags: u32,
        ) -> i32;
    }

    #[test]
    fn mode_icons_have_distinct_visible_glyphs() {
        unsafe {
            let chinese = icon_pixels(false, true).unwrap();
            let english = icon_pixels(true, true).unwrap();
            assert_ne!(chinese, english);
            for ascii in [false, true] {
                let light = icon_pixels(ascii, true).unwrap();
                let dark = icon_pixels(ascii, false).unwrap();
                assert!(light.contains(&0));
                assert!(light.iter().any(|pixel| *pixel >> 24 > 0));
                for (black, white) in light.iter().zip(dark) {
                    let alpha = black >> 24;
                    assert_eq!(black & 0x00ff_ffff, 0, "淺色背景只顯示黑色文字");
                    assert_eq!(white, (alpha << 24) | (alpha * 0x0001_0101));
                }
                for (pixels, background) in [(light, 0x00e8_e8e8), (dark, 0x0022_2222)] {
                    let icon = icon_from_pixels(&pixels);
                    assert!(!icon.is_null());
                    let mut surface = Surface::new(16, 16).unwrap();
                    surface.bitmap.pixels_mut().fill(background);
                    assert_ne!(
                        DrawIconEx(surface.dc, 0, 0, icon, 16, 16, 0, ptr::null_mut(), 3),
                        0
                    );
                    GdiFlush();
                    let result = surface.bitmap.pixels();
                    assert_eq!(result[0] & 0x00ff_ffff, background, "背景應保持透明");
                    assert!(result.iter().any(|pixel| pixel & 0x00ff_ffff != background));
                    assert_ne!(DestroyIcon(icon), 0);
                }
            }
            let icon = make_icon(false);
            assert!(!icon.is_null());
            assert_ne!(DestroyIcon(icon), 0);
        }
    }

    #[test]
    fn mode_change_notifies_registered_language_bar_sink() {
        unsafe fn noop(_: Raw) {}
        unsafe {
            let button = Box::into_raw(Box::new(Button {
                button_vtbl: &BUTTON_VTBL,
                source_vtbl: &SOURCE_VTBL,
                mode_vtbl: &MODE_VTBL,
                refs: AtomicU32::new(1),
                ascii: AtomicBool::new(true),
                owner: ptr::null_mut(),
                thread_manager: ptr::null_mut(),
                toggle: noop,
                sink: ptr::null_mut(),
            }));
            OBJECTS.fetch_add(1, Ordering::SeqCst);
            let mut bar = LanguageBar {
                manager: ptr::null_mut(),
                button: button.cast(),
                notification: None,
            };
            let mut sink = Sink {
                vtbl: &SINK_VTBL,
                refs: AtomicU32::new(1),
                calls: AtomicU32::new(0),
                flags: AtomicU32::new(0),
            };
            let source = ffi::query(button.cast(), &ffi::IID_ITfSource).unwrap();
            let mut cookie = 0;
            assert_eq!(
                (ffi::vtable::<SourceVtbl>(source).advise)(
                    source,
                    &ffi::IID_ITfLangBarItemSink,
                    (&mut sink as *mut Sink).cast(),
                    &mut cookie
                ),
                ffi::S_OK
            );
            assert_eq!(cookie, 1);
            assert_eq!(sink.refs.load(Ordering::SeqCst), 2);
            bar.set_ascii(false);
            bar.set_ascii(false);
            assert_eq!(sink.calls.load(Ordering::SeqCst), 1);
            assert_eq!(sink.flags.load(Ordering::SeqCst), 7);
            assert_eq!(
                (ffi::vtable::<SourceVtbl>(source).unadvise)(source, cookie),
                ffi::S_OK
            );
            assert_eq!(sink.refs.load(Ordering::SeqCst), 1);
            ffi::release(source);
            bar.detach();
        }
    }
}
