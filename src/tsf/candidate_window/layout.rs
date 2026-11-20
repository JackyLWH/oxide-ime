//! 量度字型、安排候選及頁碼，並計算視窗位置。
use std::ptr;

use super::super::{
    font,
    gdi::{CreateFontW, DeleteObject, SelectObject},
};
use super::placement::{fallback_rect, fit_to_monitor, fit_toast, monitor_work_area};
use super::{
    wide, CandidateWindow, GetDeviceCaps, PaintItem, PaintPage, PaintRun, Raw, Rect, ReleaseDC,
    Size, WindowContent, LOGPIXELSY,
};

#[link(name = "gdi32")]
extern "system" {
    fn GetTextExtentPoint32W(dc: Raw, text: *const u16, count: i32, size: *mut Size) -> i32;
    fn GetTextFaceW(dc: Raw, count: i32, face: *mut u16) -> i32;
    fn GetFontData(dc: Raw, table: u32, offset: u32, data: Raw, bytes: u32) -> u32;
}

pub(super) const PANEL_FONT_POINTS: i32 = 17;

pub(super) struct CandidateFont {
    pub(super) handle: Raw,
    pub(super) cmap: Vec<u8>,
}

impl Drop for CandidateFont {
    fn drop(&mut self) {
        unsafe { DeleteObject(self.handle) };
    }
}

fn candidate_runs(text: &str, fonts: &[CandidateFont], fallback: Raw) -> Vec<PaintRun> {
    let mut runs: Vec<PaintRun> = Vec::new();
    for ch in text.chars() {
        let selected = fonts
            .iter()
            .find(|face| font::supports(&face.cmap, ch))
            .map_or(fallback, |face| face.handle);
        if runs.last().is_none_or(|run| run.font != selected) {
            runs.push(PaintRun {
                text: Vec::new(),
                font: selected,
                width: 0,
            });
        }
        let mut units = [0; 2];
        runs.last_mut()
            .expect("字型片段已建立")
            .text
            .extend_from_slice(ch.encode_utf16(&mut units));
    }
    for run in &mut runs {
        run.text.push(0);
    }
    runs
}

impl CandidateWindow {
    pub(super) unsafe fn layout(
        &mut self,
        items: &[(String, bool)],
        content: WindowContent,
        dc: Raw,
    ) -> Option<(i32, i32)> {
        let vertical = matches!(content, WindowContent::Switches);
        let compact = matches!(content, WindowContent::Mode(_));
        let old = if self.model.font.is_null() {
            ptr::null_mut()
        } else {
            SelectObject(dc, self.model.font)
        };
        let dpi = GetDeviceCaps(dc, LOGPIXELSY).max(1);
        self.ensure_styled_fonts(dc, dpi);
        let (_, fonts) = self.styled_fonts.as_ref().expect("字型已初始化");
        let scale = |n: i32| (n * dpi + 48) / 96;
        let margin = scale(12).max(1);
        let gap = scale(5).max(1);
        let padding = scale(2).max(1);
        let page = if let WindowContent::Candidates(page) = content {
            let text = wide(&(page + 1).to_string());
            let previous = if self.model.page_font.is_null() {
                ptr::null_mut()
            } else {
                SelectObject(dc, self.model.page_font)
            };
            let mut size = Size::default();
            GetTextExtentPoint32W(dc, text.as_ptr(), (text.len() - 1) as i32, &mut size);
            if !previous.is_null() {
                SelectObject(dc, previous);
            }
            Some((text, size))
        } else {
            None
        };
        if self.anchor.is_none() {
            self.anchor = fallback_rect();
        }
        let work = self.anchor.and_then(|anchor| monitor_work_area(anchor));
        let page_width = page.as_ref().map_or(0, |(_, size)| size.cx + scale(7));
        let trailing = margin.max(page_width);
        let max_width = if page.is_some() {
            work.map_or(i32::MAX, |work| (work.right - work.left).max(1))
        } else {
            i32::MAX
        };
        let mut paint_items = Vec::with_capacity(items.len());
        let mut x = margin;
        let mut y = margin;
        let mut right = margin;
        let mut bottom = margin;
        let mut row_height = 0;
        for (value, selected) in items {
            let mut runs = candidate_runs(value, fonts, self.model.font);
            let (mut text_width, mut text_height) = (0, 0);
            for run in &mut runs {
                if !run.font.is_null() {
                    SelectObject(dc, run.font);
                }
                let mut size = Size::default();
                GetTextExtentPoint32W(
                    dc,
                    run.text.as_ptr(),
                    (run.text.len() - 1) as i32,
                    &mut size,
                );
                text_width += size.cx;
                text_height = text_height.max(size.cy);
                run.width = size.cx;
            }
            let item_width = (text_width + padding * 2).max(1);
            let item_height = (text_height + padding * 2).max(scale(24));
            // 先用盡當頁的橫向空間，再把其餘候選移至下一列。
            if !vertical && x > margin && x + item_width + trailing > max_width {
                x = margin;
                y += row_height + gap;
                row_height = 0;
            }
            paint_items.push(PaintItem {
                runs,
                selected: *selected,
                rect: Rect {
                    left: x,
                    top: y,
                    right: x + item_width,
                    bottom: y + item_height,
                },
            });
            right = right.max(x + item_width);
            bottom = bottom.max(y + item_height);
            row_height = row_height.max(item_height);
            if vertical {
                y += item_height + gap;
            } else {
                x += item_width + gap;
            }
        }
        if !old.is_null() {
            SelectObject(dc, old);
        }
        ReleaseDC(self.window, dc);
        let width = if vertical {
            (right + trailing).max(scale(160))
        } else {
            (right + trailing).min(max_width)
        };
        let height = bottom + margin;
        let rect = self.anchor?;
        if vertical {
            for item in &mut paint_items {
                item.rect.right = width - margin;
            }
        }
        self.model.items = paint_items;
        self.model.page = page.map(|(text, size)| {
            let right = width - scale(4).max(1);
            let bottom = height - scale(3).max(1);
            PaintPage {
                text,
                rect: Rect {
                    left: right - size.cx,
                    top: bottom - size.cy,
                    right,
                    bottom,
                },
            }
        });
        self.model.clickable_count = if compact { 0 } else { items.len() };
        self.model.width = width;
        self.model.height = height;
        self.model.padding = padding;
        self.model.border_width = scale(3).max(1);
        self.model.corner_diameter = scale(10).max(2);
        let (x, y) = if let WindowContent::Mode(avoid) = content {
            fit_toast(rect, width, height, work, avoid)?
        } else {
            let gap = if matches!(content, WindowContent::Candidates(_)) {
                scale(6).max(1)
            } else {
                0
            };
            fit_to_monitor(rect, width, height, gap, work)
        };
        Some((x, y))
    }

    unsafe fn ensure_styled_fonts(&mut self, dc: Raw, dpi: i32) {
        if self
            .styled_fonts
            .as_ref()
            .is_some_and(|(key, _)| *key == dpi)
        {
            return;
        }
        let height = -((PANEL_FONT_POINTS * dpi + 36) / 72);
        let create = |face: &str| {
            CreateFontW(
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
                4, // ANTIALIASED_QUALITY：灰階平滑，避免 ClearType 彩色子像素邊緣。
                0,
                wide(face).as_ptr(),
            )
        };
        let open = |names: &[&str]| {
            for name in names {
                let handle = create(name);
                if handle.is_null() {
                    continue;
                }
                let mut font = CandidateFont {
                    handle,
                    cmap: Vec::new(),
                };
                let old = SelectObject(dc, handle);
                if old.is_null() || old as isize == -1 {
                    continue;
                }
                let mut face = [0; 64];
                let count = GetTextFaceW(dc, face.len() as i32, face.as_mut_ptr());
                let actual = String::from_utf16_lossy(
                    &face[..face.iter().position(|&ch| ch == 0).unwrap_or(face.len())],
                );
                // GDI 在指定家族不存在時仍可能回傳一個替代字型。
                if count > 0 && names.iter().any(|name| actual.eq_ignore_ascii_case(name)) {
                    const CMAP: u32 = u32::from_le_bytes(*b"cmap");
                    let bytes = GetFontData(dc, CMAP, 0, ptr::null_mut(), 0);
                    if (1..=2_000_000).contains(&bytes) {
                        font.cmap.resize(bytes as usize, 0);
                        if GetFontData(dc, CMAP, 0, font.cmap.as_mut_ptr().cast(), bytes) != bytes {
                            font.cmap.clear();
                        }
                    }
                }
                SelectObject(dc, old);
                if !font.cmap.is_empty() {
                    return Some(font);
                }
            }
            None
        };
        let mut fonts = Vec::new();
        fonts.extend(
            open(&["TeXGyreTermes", "TeX Gyre Termes"]).or_else(|| open(&["Times New Roman"])),
        );
        fonts.extend(open(&["全字庫正楷體", "TW-Kai"]));
        fonts.extend(open(&["KaiTi", "楷体", "SimKai"]));
        fonts.extend(open(&["標楷體", "DFKai-SB"]));
        self.styled_fonts = Some((dpi, fonts));
    }
}
