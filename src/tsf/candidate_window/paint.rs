//! 候選框的雙緩衝繪圖和圓角平滑。
use std::ptr;

use super::super::gdi::{
    CreateCompatibleDC, DeleteDC, DeleteObject, DrawTextW, GdiFlush, SelectObject, SetBkMode,
    SetTextColor, Surface,
};
use super::{PaintModel, Point, Raw, Rect, Size};

pub(super) const COLOR_BACKGROUND: u32 = 0x00ff_ffff;
pub(super) const COLOR_BORDER: u32 = 0x00e2_e2e2;
pub(super) const COLOR_HIGHLIGHT: u32 = 0x00ce_7539; // Weasel 的 BGR 色碼。
const COLOR_TEXT: u32 = 0;
const COLOR_PAGE: u32 = 0x0090_9090;
const SRCCOPY: u32 = 0x00cc_0020;
const SUPERSAMPLE: i32 = 3;

#[repr(C)]
struct BlendFunction {
    operation: u8,
    flags: u8,
    alpha: u8,
    alpha_format: u8,
}

#[link(name = "user32")]
extern "system" {
    fn UpdateLayeredWindow(
        window: Raw,
        dest_dc: Raw,
        dest: *const Point,
        size: *const Size,
        source_dc: Raw,
        source: *const Point,
        color_key: u32,
        blend: *const BlendFunction,
        flags: u32,
    ) -> i32;
    fn FillRect(dc: Raw, rect: *const Rect, brush: Raw) -> i32;
}

#[link(name = "gdi32")]
extern "system" {
    fn CreateSolidBrush(color: u32) -> Raw;
    fn CreatePen(style: i32, width: i32, color: u32) -> Raw;
    fn GetStockObject(index: i32) -> Raw;
    fn CreateCompatibleBitmap(dc: Raw, width: i32, height: i32) -> Raw;
    fn SetMapMode(dc: Raw, mode: i32) -> i32;
    fn SetWindowExtEx(dc: Raw, x: i32, y: i32, old: *mut Size) -> i32;
    fn SetViewportExtEx(dc: Raw, x: i32, y: i32, old: *mut Size) -> i32;
    fn BitBlt(
        dest: Raw,
        x: i32,
        y: i32,
        width: i32,
        height: i32,
        source: Raw,
        source_x: i32,
        source_y: i32,
        rop: u32,
    ) -> i32;
    fn RoundRect(
        dc: Raw,
        left: i32,
        top: i32,
        right: i32,
        bottom: i32,
        width: i32,
        height: i32,
    ) -> i32;
}

unsafe fn fill_round_rect(dc: Raw, rect: &Rect, radius: i32, color: u32) {
    let brush = CreateSolidBrush(color);
    let pen = GetStockObject(8); // NULL_PEN：不要讓筆畫伸出選取區。
    if brush.is_null() || pen.is_null() {
        if !brush.is_null() {
            DeleteObject(brush);
        }
        return;
    }
    let old_brush = SelectObject(dc, brush);
    let old_pen = SelectObject(dc, pen);
    RoundRect(
        dc,
        rect.left,
        rect.top,
        rect.right,
        rect.bottom,
        radius,
        radius,
    );
    SelectObject(dc, old_pen);
    SelectObject(dc, old_brush);
    DeleteObject(brush);
}

unsafe fn paint_background(dc: Raw, model: &PaintModel) {
    let bounds = Rect {
        left: 0,
        top: 0,
        right: model.width,
        bottom: model.height,
    };
    let background = CreateSolidBrush(COLOR_BACKGROUND);
    FillRect(dc, &bounds, background);
    let outline = CreatePen(0, model.border_width, COLOR_BORDER);
    let old_pen = SelectObject(dc, outline);
    let old_brush = SelectObject(dc, background);
    let inset = model.border_width / 2;
    RoundRect(
        dc,
        inset,
        inset,
        model.width - inset,
        model.height - inset,
        model.corner_diameter,
        model.corner_diameter,
    );
    SelectObject(dc, old_brush);
    SelectObject(dc, old_pen);
    DeleteObject(outline);
    // 先畫選取底色，避免後一項的底色蓋住前一項的文字。
    for item in model.items.iter().filter(|item| item.selected) {
        fill_round_rect(dc, &item.rect, model.corner_diameter, COLOR_HIGHLIGHT);
    }
    DeleteObject(background);
}

unsafe fn paint_items(dc: Raw, model: &PaintModel, mask: bool) {
    let mut old_font = if model.font.is_null() {
        ptr::null_mut()
    } else {
        SelectObject(dc, model.font)
    };
    SetBkMode(dc, 1); // TRANSPARENT
    for item in &model.items {
        SetTextColor(
            dc,
            if mask || item.selected {
                COLOR_BACKGROUND
            } else {
                COLOR_TEXT
            },
        );
        let mut x = item.rect.left + model.padding;
        for run in &item.runs {
            if !run.font.is_null() {
                let previous = SelectObject(dc, run.font);
                if old_font.is_null() {
                    old_font = previous;
                }
            }
            let mut text_rect = Rect {
                left: x,
                top: item.rect.top,
                right: x + run.width,
                bottom: item.rect.bottom,
            };
            DrawTextW(
                dc,
                run.text.as_ptr(),
                (run.text.len() - 1) as i32,
                &mut text_rect,
                0x0000_0020 | 0x0000_0004 | 0x0000_0800,
            );
            x += run.width;
        }
    }
    if !old_font.is_null() {
        SelectObject(dc, old_font);
    }
}

unsafe fn paint_page(dc: Raw, model: &PaintModel) {
    let Some(page) = &model.page else { return };
    let font = if model.page_font.is_null() {
        model.font
    } else {
        model.page_font
    };
    let previous = if font.is_null() {
        ptr::null_mut()
    } else {
        SelectObject(dc, font)
    };
    SetBkMode(dc, 1);
    SetTextColor(dc, COLOR_PAGE);
    let mut rect = page.rect;
    DrawTextW(
        dc,
        page.text.as_ptr(),
        (page.text.len() - 1) as i32,
        &mut rect,
        0x0000_0020 | 0x0000_0002 | 0x0000_0008 | 0x0000_0800,
    );
    if !previous.is_null() {
        SelectObject(dc, previous);
    }
}

unsafe fn paint_contents(dc: Raw, model: &PaintModel) {
    paint_background(dc, model);
    paint_items(dc, model, false);
    paint_page(dc, model);
}

pub(super) unsafe fn paint_buffered(dc: Raw, model: &PaintModel) {
    let memory = CreateCompatibleDC(dc);
    if memory.is_null() {
        paint_contents(dc, model);
        return;
    }
    let bitmap = CreateCompatibleBitmap(dc, model.width, model.height);
    if bitmap.is_null() {
        DeleteDC(memory);
        paint_contents(dc, model);
        return;
    }
    let previous = SelectObject(memory, bitmap);
    if previous.is_null() || previous as isize == -1 {
        DeleteObject(bitmap);
        DeleteDC(memory);
        paint_contents(dc, model);
        return;
    }
    paint_contents(memory, model);
    let copied = BitBlt(dc, 0, 0, model.width, model.height, memory, 0, 0, SRCCOPY);
    SelectObject(memory, previous);
    DeleteObject(bitmap);
    DeleteDC(memory);
    if copied == 0 {
        paint_contents(dc, model);
    }
}

fn inside_round_rect(x: i32, y: i32, width: i32, height: i32, radius: i32) -> bool {
    if radius <= 0 {
        return true;
    }
    let x = x * 2 + 1;
    let y = y * 2 + 1;
    let radius = radius * 2;
    let dx = if x < radius {
        radius - x
    } else {
        (x - (width * 2 - radius)).max(0)
    };
    let dy = if y < radius {
        radius - y
    } else {
        (y - (height * 2 - radius)).max(0)
    };
    (dx as i64) * (dx as i64) + (dy as i64) * (dy as i64) <= (radius as i64) * (radius as i64)
}

fn composite_text(model: &PaintModel, mask: &[u32], output: &mut [u32]) {
    let scale = SUPERSAMPLE as usize;
    let width = model.width as usize;
    let stride = width * scale;
    let weight = (3 * scale * scale) as u32;
    // 取單一色頻作覆蓋率，讓後備字型也以灰階而非彩色子像素混合。
    let gray = |pixel: u32| (pixel >> 8) & 255;
    for item in &model.items {
        let left = item.rect.left.max(1) as usize;
        let right = item.rect.right.clamp(0, model.width) as usize;
        // 上下各保留一列供字形擴張。文字區本身有邊距，不接觸視窗邊緣。
        let top = item.rect.top.max(1) as usize;
        let bottom = item.rect.bottom.clamp(0, model.height - 1) as usize;
        for y in top..bottom {
            for x in left..right {
                let mut sum = 0;
                for sample_y in y * scale..(y + 1) * scale {
                    let row = sample_y * stride;
                    for sample_x in x * scale..(x + 1) * scale {
                        let index = row + sample_x;
                        let center = gray(mask[index]);
                        let east = row + (sample_x + 1).min(stride - 1);
                        let expanded = center
                            .max(gray(mask[index - 1]))
                            .max(gray(mask[east]))
                            .max(gray(mask[index - stride]))
                            .max(gray(mask[index + stride]));
                        // 擴張樣本佔三分之一，保留灰階邊緣與較細的筆畫。
                        sum += 2 * center + expanded;
                    }
                }
                let coverage = (sum + weight / 2) / weight;
                if coverage == 0 {
                    continue;
                }
                let pixel = &mut output[y * width + x];
                let alpha = *pixel >> 24;
                let ink = if item.selected { alpha } else { 0 };
                let blend = |shift: u32| {
                    let base = (*pixel >> shift) & 255;
                    (base * (255 - coverage) + ink * coverage + 127) / 255
                };
                *pixel = (alpha << 24) | (blend(16) << 16) | (blend(8) << 8) | blend(0);
            }
        }
    }
}

pub(super) unsafe fn render_surface(model: &PaintModel) -> Option<Surface> {
    let width = model.width.checked_mul(SUPERSAMPLE)?;
    let height = model.height.checked_mul(SUPERSAMPLE)?;
    let mut source = Surface::new(width, height)?;
    if SetMapMode(source.dc, 8) == 0 // MM_ANISOTROPIC
        || SetWindowExtEx(source.dc, 1, 1, ptr::null_mut()) == 0
        || SetViewportExtEx(source.dc, SUPERSAMPLE, SUPERSAMPLE, ptr::null_mut()) == 0
    {
        return None;
    }
    paint_background(source.dc, model);
    paint_page(source.dc, model);
    GdiFlush();
    let mut output = Surface::new(model.width, model.height)?;
    let source_pixels = source.bitmap.pixels();
    let output_pixels = output.bitmap.pixels_mut();
    let radius = (model.corner_diameter * SUPERSAMPLE / 2)
        .min(width / 2)
        .min(height / 2);
    let samples = (SUPERSAMPLE * SUPERSAMPLE) as u32;
    for y in 0..model.height {
        let top = y * SUPERSAMPLE;
        let corner_row = top < radius || top + SUPERSAMPLE > height - radius;
        for x in 0..model.width {
            let left = x * SUPERSAMPLE;
            let corner = corner_row && (left < radius || left + SUPERSAMPLE > width - radius);
            let (mut alpha, mut red, mut green, mut blue) = (0u32, 0u32, 0u32, 0u32);
            for sy in 0..SUPERSAMPLE {
                for sx in 0..SUPERSAMPLE {
                    let px = left + sx;
                    let py = top + sy;
                    if corner && !inside_round_rect(px, py, width, height, radius) {
                        continue;
                    }
                    let color = source_pixels[(py * width + px) as usize];
                    alpha += 255;
                    red += (color >> 16) & 255;
                    green += (color >> 8) & 255;
                    blue += color & 255;
                }
            }
            // UpdateLayeredWindow 要求各色彩預乘 alpha；透明區域不混入白色背景。
            output_pixels[(y * model.width + x) as usize] = ((alpha / samples) << 24)
                | ((red / samples) << 16)
                | ((green / samples) << 8)
                | (blue / samples);
        }
    }
    // 候選、F4 及狀態浮標共用灰階字形，圓角及頁碼各自繪製。
    source.bitmap.pixels_mut().fill(0);
    paint_items(source.dc, model, true);
    GdiFlush();
    composite_text(model, source.bitmap.pixels(), output_pixels);
    Some(output)
}

pub(super) unsafe fn paint_layered(window: Raw, model: &PaintModel, x: i32, y: i32) -> bool {
    let Some(surface) = render_surface(model) else {
        return false;
    };
    let dest = Point { x, y };
    let size = Size {
        cx: model.width,
        cy: model.height,
    };
    let source = Point { x: 0, y: 0 };
    let blend = BlendFunction {
        operation: 0,
        flags: 0,
        alpha: 255,
        alpha_format: 1,
    };
    UpdateLayeredWindow(
        window,
        ptr::null_mut(),
        &dest,
        &size,
        surface.dc,
        &source,
        0,
        &blend,
        2,
    ) != 0
}
