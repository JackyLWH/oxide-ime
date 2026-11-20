//! 候選窗及工作列圖示共用的 Windows 繪字及 32 位元點陣圖。
use std::mem::offset_of;
use std::ptr;

use super::ffi::{Raw, Rect};

#[repr(C)]
struct BitmapInfo {
    size: u32,
    width: i32,
    height: i32,
    planes: u16,
    bit_count: u16,
    compression: u32,
    image_size: u32,
    x_pixels_per_meter: i32,
    y_pixels_per_meter: i32,
    colors_used: u32,
    colors_important: u32,
    color: u32,
}

#[link(name = "gdi32")]
extern "system" {
    pub(super) fn CreateCompatibleDC(dc: Raw) -> Raw;
    pub(super) fn DeleteDC(dc: Raw) -> i32;
    pub(super) fn SelectObject(dc: Raw, object: Raw) -> Raw;
    pub(super) fn DeleteObject(object: Raw) -> i32;
    pub(super) fn GdiFlush() -> i32;
    pub(super) fn CreateFontW(
        height: i32,
        width: i32,
        escapement: i32,
        orientation: i32,
        weight: i32,
        italic: u32,
        underline: u32,
        strikeout: u32,
        charset: u32,
        out_precision: u32,
        clip_precision: u32,
        quality: u32,
        pitch_and_family: u32,
        face: *const u16,
    ) -> Raw;
    pub(super) fn SetTextColor(dc: Raw, color: u32) -> u32;
    pub(super) fn SetBkMode(dc: Raw, mode: i32) -> i32;
    fn CreateDIBSection(
        dc: Raw,
        info: *const BitmapInfo,
        usage: u32,
        bits: *mut Raw,
        section: Raw,
        offset: u32,
    ) -> Raw;
}

#[link(name = "user32")]
extern "system" {
    pub(super) fn DrawTextW(
        dc: Raw,
        text: *const u16,
        len: i32,
        rect: *mut Rect,
        format: u32,
    ) -> i32;
}

pub(super) struct Dib {
    pub(super) handle: Raw,
    bits: *mut u32,
    len: usize,
}

impl Dib {
    pub(super) unsafe fn new(dc: Raw, width: i32, height: i32) -> Option<Self> {
        if width <= 0 || height <= 0 || i64::from(width) * i64::from(height) > 32_000_000 {
            return None;
        }
        let info = BitmapInfo {
            size: offset_of!(BitmapInfo, color) as u32,
            width,
            height: -height,
            planes: 1,
            bit_count: 32,
            compression: 0, // BI_RGB
            image_size: 0,
            x_pixels_per_meter: 0,
            y_pixels_per_meter: 0,
            colors_used: 0,
            colors_important: 0,
            color: 0,
        };
        let mut bits = ptr::null_mut();
        let handle = CreateDIBSection(dc, &info, 0, &mut bits, ptr::null_mut(), 0);
        if handle.is_null() || bits.is_null() {
            if !handle.is_null() {
                DeleteObject(handle);
            }
            return None;
        }
        let len = (width * height) as usize;
        let bits = bits.cast::<u32>();
        ptr::write_bytes(bits, 0, len);
        Some(Self { handle, bits, len })
    }

    /// 呼叫端須先以 GdiFlush 完成所有繪圖，再讀寫像素。
    pub(super) unsafe fn pixels(&self) -> &[u32] {
        std::slice::from_raw_parts(self.bits, self.len)
    }

    pub(super) unsafe fn pixels_mut(&mut self) -> &mut [u32] {
        std::slice::from_raw_parts_mut(self.bits, self.len)
    }
}

impl Drop for Dib {
    fn drop(&mut self) {
        unsafe { DeleteObject(self.handle) };
    }
}

pub(super) struct Surface {
    pub(super) dc: Raw,
    pub(super) bitmap: Dib,
    previous: Raw,
}

impl Surface {
    pub(super) unsafe fn new(width: i32, height: i32) -> Option<Self> {
        let dc = CreateCompatibleDC(ptr::null_mut());
        if dc.is_null() {
            return None;
        }
        let Some(bitmap) = Dib::new(dc, width, height) else {
            DeleteDC(dc);
            return None;
        };
        let previous = SelectObject(dc, bitmap.handle);
        if previous.is_null() || previous as isize == -1 {
            DeleteDC(dc);
            return None;
        }
        Some(Self {
            dc,
            bitmap,
            previous,
        })
    }
}

impl Drop for Surface {
    fn drop(&mut self) {
        unsafe {
            SelectObject(self.dc, self.previous);
            DeleteDC(self.dc);
        }
    }
}
