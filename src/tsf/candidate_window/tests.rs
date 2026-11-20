use super::paint::{render_surface, COLOR_BACKGROUND, COLOR_BORDER, COLOR_HIGHLIGHT};
use super::placement::fit_to_monitor;
use super::*;
use crate::tsf::font;
use crate::tsf::gdi::GdiFlush;
use crate::win32::GetCurrentThreadId;
#[repr(C)]
struct Message {
    window: Raw,
    message: u32,
    wparam: usize,
    lparam: isize,
    time: u32,
    point: Point,
    private: u32,
}
#[repr(C)]
struct LogFont {
    height: i32,
    width: i32,
    escapement: i32,
    orientation: i32,
    weight: i32,
    attributes: [u8; 8],
    face: [u16; 32],
}
#[link(name = "user32")]
extern "system" {
    fn CreateDesktopW(
        name: *const u16,
        device: Raw,
        devmode: Raw,
        flags: u32,
        access: u32,
        security: Raw,
    ) -> Raw;
    fn GetThreadDesktop(thread: u32) -> Raw;
    fn SetThreadDesktop(desktop: Raw) -> i32;
    fn SetFocus(window: Raw) -> Raw;
    fn CloseDesktop(desktop: Raw) -> i32;
    fn GetWindowTextW(window: Raw, text: *mut u16, capacity: i32) -> i32;
    fn GetUpdateRect(window: Raw, rect: *mut Rect, erase: i32) -> i32;
    fn GetGuiResources(process: Raw, flags: u32) -> u32;
    fn PeekMessageW(message: *mut Message, window: Raw, min: u32, max: u32, remove: u32) -> i32;
    fn DispatchMessageW(message: *const Message) -> isize;
    fn SendMessageW(window: Raw, message: u32, wparam: usize, lparam: isize) -> isize;
}
#[link(name = "gdi32")]
extern "system" {
    fn GetPixel(dc: Raw, x: i32, y: i32) -> u32;
    fn GetObjectW(object: Raw, size: i32, data: Raw) -> i32;
}
#[link(name = "kernel32")]
extern "system" {
    fn GetCurrentProcess() -> Raw;
}

unsafe fn capture_if_requested(window: &CandidateWindow, name: &str) {
    let Some(requested) = std::env::var_os("RUST_IME_CAPTURE") else {
        return;
    };
    if requested != "1" && requested != name {
        return;
    }
    UpdateWindow(window.window);
    let width = window.model.width as u32;
    let height = window.model.height as u32;
    let stride = (width * 3 + 3) & !3;
    let mut bytes = vec![0u8; (54 + stride * height) as usize];
    bytes[0..2].copy_from_slice(b"BM");
    let length = bytes.len() as u32;
    bytes[2..6].copy_from_slice(&length.to_le_bytes());
    bytes[10..14].copy_from_slice(&54u32.to_le_bytes());
    bytes[14..18].copy_from_slice(&40u32.to_le_bytes());
    bytes[18..22].copy_from_slice(&width.to_le_bytes());
    bytes[22..26].copy_from_slice(&height.to_le_bytes());
    bytes[26..28].copy_from_slice(&1u16.to_le_bytes());
    bytes[28..30].copy_from_slice(&24u16.to_le_bytes());
    let surface = render_surface(&window.model).expect("無法建立測試畫面");
    let pixels = surface.bitmap.pixels();
    for y in 0..height {
        for x in 0..width {
            let color = pixels[((height - 1 - y) * width + x) as usize];
            let background = 255 - (color >> 24);
            let index = (54 + y * stride + x * 3) as usize;
            bytes[index] = ((color & 255) + background) as u8;
            bytes[index + 1] = (((color >> 8) & 255) + background) as u8;
            bytes[index + 2] = (((color >> 16) & 255) + background) as u8;
        }
    }
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("work");
    std::fs::write(root.join(name), bytes).unwrap();
}
#[test]
fn accessible_label_contains_the_page_without_an_arrow() {
    let views = vec![
        CandidateView {
            text: "日".into(),
            selected: true,
        },
        CandidateView {
            text: "長詞…".into(),
            selected: false,
        },
    ];
    let items = candidate_items(&views);
    assert_eq!(candidate_label(&items, 0), "1. 日   2. 長詞…   第 1 頁");
    assert_eq!(candidate_label(&items, 12), "1. 日   2. 長詞…   第 13 頁");
}

#[test]
fn candidate_gap_respects_screen_edges() {
    let work = Rect {
        left: 0,
        top: 0,
        right: 500,
        bottom: 300,
    };
    let anchor = Rect {
        left: 40,
        top: 40,
        right: 41,
        bottom: 60,
    };
    assert_eq!(fit_to_monitor(anchor, 100, 40, 6, Some(work)), (40, 66));
    let near_bottom = Rect {
        left: 40,
        top: 270,
        right: 41,
        bottom: 290,
    };
    assert_eq!(
        fit_to_monitor(near_bottom, 100, 40, 6, Some(work)),
        (40, 224)
    );
    let cramped = Rect {
        left: 40,
        top: 30,
        right: 41,
        bottom: 270,
    };
    assert_eq!(fit_to_monitor(cramped, 480, 80, 6, Some(work)), (20, 220));
}

#[test]
fn toast_fits_around_candidates_at_the_top_and_right_edge() {
    let work = Rect {
        left: -500,
        top: 0,
        right: 0,
        bottom: 300,
    };
    let anchor = Rect {
        left: -15,
        top: 0,
        right: -14,
        bottom: 24,
    };
    let right_aligned = Rect {
        left: -160,
        top: 30,
        right: 0,
        bottom: 76,
    };
    assert_eq!(
        fit_toast(anchor, 64, 36, Some(work), Some(right_aligned)),
        Some((-228, 28))
    );
    let full_width = Rect {
        left: work.left,
        ..right_aligned
    };
    assert_eq!(
        fit_toast(anchor, 64, 36, Some(work), Some(full_width)),
        Some((-64, 80))
    );
    assert_eq!(fit_toast(anchor, 64, 36, Some(work), Some(work)), None);
}

#[test]
#[ignore = "需要 Windows 互動工作站；只在獨立且不可見的測試桌面建立視窗"]
fn candidate_window_shows_and_hides_without_activation() {
    unsafe fn record_click(owner: Raw, index: usize) {
        (*(owner as *mut std::sync::atomic::AtomicUsize))
            .store(index + 1, std::sync::atomic::Ordering::SeqCst);
    }
    struct Desktop {
        old: Raw,
        test: Raw,
    }
    impl Drop for Desktop {
        fn drop(&mut self) {
            unsafe {
                SetThreadDesktop(self.old);
                CloseDesktop(self.test);
            }
        }
    }
    unsafe {
        let old = GetThreadDesktop(GetCurrentThreadId());
        let name = wide(&format!("rust-ime-ui-test-{}", std::process::id()));
        let test = CreateDesktopW(
            name.as_ptr(),
            ptr::null_mut(),
            ptr::null_mut(),
            0,
            0x000f_01ff,
            ptr::null_mut(),
        );
        assert!(!old.is_null() && !test.is_null(), "無法建立隔離桌面");
        assert_ne!(SetThreadDesktop(test), 0, "無法切換測試執行緒桌面");
        let _desktop = Desktop { old, test };
        let host = CreateWindowExW(
            0,
            wide("STATIC").as_ptr(),
            wide("隔離宿主").as_ptr(),
            WS_POPUP | 0x1000_0000 | SS_NOTIFY,
            50,
            50,
            200,
            100,
            ptr::null_mut(),
            ptr::null_mut(),
            ptr::null_mut(),
            ptr::null_mut(),
        );
        assert!(!host.is_null(), "無法建立隔離宿主視窗");
        SetFocus(host);
        assert_eq!(GetFocus(), host, "隔離宿主未取得焦點");
        let mut window = CandidateWindow::new();
        assert!(window.ensure_window(), "無法建立候選視窗");
        assert_eq!(
            GetWindowLongPtrW(window.window, GWLP_HWNDPARENT),
            host as isize,
            "候選視窗建立時須以目前輸入視窗作為擁有者"
        );
        let clicked = std::sync::atomic::AtomicUsize::new(0);
        window.set_click_handler(
            (&clicked as *const std::sync::atomic::AtomicUsize)
                .cast_mut()
                .cast(),
            record_click,
        );
        let candidates = [
            CandidateView {
                text: "日".into(),
                selected: true,
            },
            CandidateView {
                text: "曰".into(),
                selected: false,
            },
        ];
        window.update(
            &candidates,
            0,
            Some(Rect {
                left: 100,
                top: 100,
                right: 101,
                bottom: 120,
            }),
        );
        assert!(!window.window.is_null());
        assert_ne!(IsWindowVisible(window.window), 0);
        assert_ne!(
            GetWindowLongPtrW(window.window, -20) & WS_EX_NOACTIVATE as isize,
            0
        );
        let mut text = [0u16; 128];
        let len = GetWindowTextW(window.window, text.as_mut_ptr(), text.len() as i32);
        assert_eq!(
            String::from_utf16_lossy(&text[..len as usize]),
            "1. 日   2. 曰   第 1 頁"
        );
        let page = window.model.page.as_ref().unwrap();
        assert_eq!(
            String::from_utf16_lossy(&page.text[..page.text.len() - 1]),
            "1"
        );
        assert!(page.rect.left >= window.model.items[1].rect.right);
        assert!(page.rect.bottom < window.model.height);
        assert!(page.rect.top > window.model.items[1].rect.top);
        let candidate_rect = window.model.items[1].rect;
        assert!(page.rect.bottom - page.rect.top < candidate_rect.bottom - candidate_rect.top);
        let dc = GetDC(window.window);
        assert!(!dc.is_null());
        assert!(window.model.layered, "應啟用透明圓角繪製");
        let (key, fonts) = window.styled_fonts.as_ref().unwrap();
        let font_dpi = GetDeviceCaps(dc, LOGPIXELSY);
        assert_eq!(*key, font_dpi);
        for font in fonts {
            let mut logical: LogFont = std::mem::zeroed();
            let size = std::mem::size_of::<LogFont>() as i32;
            assert_eq!(
                GetObjectW(font.handle, size, (&mut logical as *mut LogFont).cast()),
                size
            );
            assert_eq!(logical.weight, 400, "候選字體應使用正常粗幼");
            assert_eq!(logical.height, -((PANEL_FONT_POINTS * font_dpi + 36) / 72));
        }
        let runs = &window.model.items[0].runs;
        assert_eq!(runs.len(), 2);
        assert_eq!(
            runs[0].font,
            fonts
                .iter()
                .find(|face| font::supports(&face.cmap, '1'))
                .map_or(window.model.font, |face| face.handle)
        );
        assert_eq!(
            runs[1].font,
            fonts
                .iter()
                .find(|face| font::supports(&face.cmap, '日'))
                .map_or(window.model.font, |face| face.handle)
        );
        ReleaseDC(window.window, dc);
        let surface = render_surface(&window.model).expect("無法繪製測試畫面");
        let dc = surface.dc;
        let pixels = surface.bitmap.pixels();
        let mut transparent = false;
        let mut partial = false;
        for y in 0..window.model.corner_diameter {
            for x in 0..window.model.corner_diameter {
                let alpha = pixels[(y * window.model.width + x) as usize] >> 24;
                transparent |= alpha == 0;
                partial |= alpha > 0 && alpha < 255;
            }
        }
        assert!(transparent && partial, "外框圓角應包含透明及平滑邊緣");
        assert_eq!(GetPixel(dc, window.model.width / 2, 1), COLOR_BORDER);
        let selected = &window.model.items[0].rect;
        assert_eq!(
            GetPixel(dc, (selected.left + selected.right) / 2, selected.top + 1),
            COLOR_HIGHLIGHT
        );
        let center = (selected.left + selected.right) / 2;
        assert_eq!(GetPixel(dc, center, selected.top - 1), COLOR_BACKGROUND);
        assert_eq!(GetPixel(dc, center, selected.top - 2), COLOR_BACKGROUND);
        let corner_size = (window.model.corner_diameter + 1) / 2;
        for (right, bottom) in [(false, false), (true, false), (true, true), (false, true)] {
            let mut smoothed = false;
            for dy in 0..corner_size {
                for dx in 0..corner_size {
                    let x = if right {
                        selected.right - 1 - dx
                    } else {
                        selected.left + dx
                    };
                    let y = if bottom {
                        selected.bottom - 1 - dy
                    } else {
                        selected.top + dy
                    };
                    let pixel = GetPixel(dc, x, y);
                    let (r, g, b) = (pixel & 255, (pixel >> 8) & 255, (pixel >> 16) & 255);
                    smoothed |= r > 0x39 && r < 255 && g > 0x75 && g < 255 && b > 0xce && b < 255;
                }
            }
            assert!(
                smoothed,
                "選取框四角應有平滑邊緣：right={right}, bottom={bottom}"
            );
        }
        let unselected = &window.model.items[1].rect;
        let mut has_smoothed_text = false;
        for y in unselected.top..unselected.bottom {
            for x in unselected.left..unselected.right {
                let pixel = GetPixel(dc, x, y);
                let (red, green, blue) = (pixel & 255, (pixel >> 8) & 255, (pixel >> 16) & 255);
                assert_eq!((red, green), (green, blue), "文字不應出現彩色子像素");
                has_smoothed_text |= red > 0 && red < 255;
            }
        }
        assert!(has_smoothed_text, "候選文字應保留灰階平滑邊緣");
        let mut initial_rect = Rect::default();
        assert_ne!(GetWindowRect(window.window, &mut initial_rect), 0);
        let dc = GetDC(window.window);
        let dpi = GetDeviceCaps(dc, LOGPIXELSY).max(1);
        ReleaseDC(window.window, dc);
        assert_eq!(initial_rect.top, 120 + ((6 * dpi + 48) / 96).max(1));
        let item = window.model.items[0].rect;
        let screen_x = initial_rect.left + (item.left + item.right) / 2;
        let screen_y = initial_rect.top + (item.top + item.bottom) / 2;
        let position = ((screen_y as u16 as usize) << 16) | screen_x as u16 as usize;
        assert_eq!(
            SendMessageW(window.window, 0x0084, 0, position as isize),
            1,
            "候選必須能通過 Windows 的滑鼠點擊位置測試"
        );
        assert_eq!(
            SendMessageW(
                window.window,
                0x0021,
                0,
                (WM_LBUTTONDOWN as isize) << 16 | 1
            ),
            3,
            "點擊候選時不可搶走文字區焦點"
        );
        let changed = [CandidateView {
            text: "較長的候選文字".into(),
            selected: true,
        }];
        window.update(
            &changed,
            0,
            Some(Rect {
                left: 300,
                top: 300,
                right: 301,
                bottom: 320,
            }),
        );
        let mut moved_rect = Rect::default();
        assert_ne!(GetWindowRect(window.window, &mut moved_rect), 0);
        assert_eq!(
            (initial_rect.left, initial_rect.top),
            (moved_rect.left, moved_rect.top)
        );
        window.update(&candidates, 0, None);
        capture_if_requested(&window, "stage18-candidates.bmp");
        if std::env::var_os("RUST_IME_CAPTURE").is_some() {
            for (selected, name) in [
                (true, "candidate-selected.bmp"),
                (false, "candidate-unselected.bmp"),
            ] {
                let sample = [CandidateView {
                    text: "「輸入錯誤」".into(),
                    selected,
                }];
                window.update(&sample, 0, None);
                capture_if_requested(&window, name);
            }
            let sample = [
                CandidateView {
                    text: "你".into(),
                    selected: false,
                },
                CandidateView {
                    text: "氣".into(),
                    selected: true,
                },
            ];
            window.update(&sample, 0, None);
            capture_if_requested(&window, "stage18-sample.bmp");
            window.update(&candidates, 0, None);
        }
        let initial_show_events = window.model.show_events.get();
        let initial_hide_events = window.model.hide_events.get();
        for index in 0..20 {
            let paints_before = window.model.paint_events.get();
            window.update(
                if index % 2 == 0 {
                    &changed
                } else {
                    &candidates
                },
                0,
                None,
            );
            assert!(
                window.model.paint_events.get() > paints_before,
                "更新後應立即重繪"
            );
            assert_eq!(GetUpdateRect(window.window, ptr::null_mut(), 0), 0);
            let mut resized = Rect::default();
            assert_ne!(GetWindowRect(window.window, &mut resized), 0);
            assert_eq!(
                (resized.left, resized.top),
                (initial_rect.left, initial_rect.top)
            );
            assert_eq!(window.model.show_events.get(), initial_show_events);
            assert_eq!(window.model.hide_events.get(), initial_hide_events);
        }
        for page_number in [1, 2, 130, 1, 2, 1] {
            window.update(&candidates, page_number - 1, None);
            assert_ne!(IsWindowVisible(window.window), 0);
            assert_eq!(window.model.show_events.get(), initial_show_events);
            assert_eq!(window.model.hide_events.get(), initial_hide_events);
            let page = window.model.page.as_ref().unwrap();
            assert_eq!(
                String::from_utf16_lossy(&page.text[..page.text.len() - 1]),
                page_number.to_string()
            );
            assert!(page.rect.left >= window.model.items[1].rect.right);
        }
        let stress_pages = |window: &mut CandidateWindow| {
            for page in 0..200 {
                window.update(&candidates, page % 3, None);
                UpdateWindow(window.window);
            }
            GdiFlush();
            GetGuiResources(GetCurrentProcess(), 0)
        };
        // 先用相同頁碼走一輪，讓 Windows 字型和分層視窗快取進入穩定狀態。
        let gdi_before = stress_pages(&mut window);
        let gdi_after = stress_pages(&mut window);
        assert!(
            gdi_before > 0 && gdi_after <= gdi_before + 1,
            "GDI 物件數 {gdi_before} → {gdi_after}"
        );
        assert_eq!(window.model.show_events.get(), initial_show_events);
        assert_eq!(window.model.hide_events.get(), initial_hide_events);
        let original = format!("{}\n第二行", "長".repeat(30));
        let long = [CandidateView {
            text: crate::engine::preview(&original, crate::dictionary::InputMode::Cangjie),
            selected: true,
        }];
        window.update(&long, 0, None);
        let len = GetWindowTextW(window.window, text.as_mut_ptr(), text.len() as i32);
        let displayed = String::from_utf16_lossy(&text[..len as usize]);
        assert!(displayed.contains('…'));
        assert!(!displayed.contains('\n') && !displayed.contains(&original));
        let work = monitor_work_area(initial_rect).expect("找不到測試螢幕");
        let edge_anchor = Rect {
            left: work.right - 32,
            right: work.right - 31,
            top: work.top + 30,
            bottom: work.top + 50,
        };
        window.reset();
        window.update(&changed, 0, Some(edge_anchor));
        let mut narrow = Rect::default();
        assert_ne!(GetWindowRect(window.window, &mut narrow), 0);
        window.update(&long, 0, None);
        let mut wide = Rect::default();
        assert_ne!(GetWindowRect(window.window, &mut wide), 0);
        assert!(wide.left < narrow.left && wide.left >= work.left && wide.right <= work.right);
        assert_eq!(wide.top, narrow.top);
        window.reset();
        let border_width = window.model.border_width;
        let corner_diameter = window.model.corner_diameter;
        let switches = [
            SwitchView {
                name: "繁簡",
                state: "繁",
                selected: false,
            },
            SwitchView {
                name: "字集",
                state: "香港字",
                selected: true,
            },
            SwitchView {
                name: "預設輸入模式",
                state: "中文",
                selected: false,
            },
            SwitchView {
                name: "字形",
                state: "半角",
                selected: false,
            },
            SwitchView {
                name: "標點",
                state: "．，",
                selected: false,
            },
        ];
        window.update_switches(&switches, None);
        assert_eq!(window.styled_fonts.as_ref().unwrap().0, font_dpi);
        assert_eq!(window.model.border_width, border_width);
        assert_eq!(window.model.corner_diameter, corner_diameter);
        let len = GetWindowTextW(window.window, text.as_mut_ptr(), text.len() as i32);
        assert!(String::from_utf16_lossy(&text[..len as usize]).contains("香港字"));
        assert!(window.model.items[1].rect.top > window.model.items[0].rect.top);
        let second = window.model.items[1].rect;
        let position = ((second.top + 2) as isize) << 16 | (second.left + 2) as isize;
        SendMessageW(window.window, WM_LBUTTONDOWN, 0, position);
        assert_eq!(clicked.load(std::sync::atomic::Ordering::SeqCst), 2);
        capture_if_requested(&window, "stage18-options.bmp");
        window.show_toast(
            "中",
            Some(Rect {
                left: 100,
                top: 100,
                right: 101,
                bottom: 120,
            }),
            None,
        );
        let len = GetWindowTextW(window.window, text.as_mut_ptr(), text.len() as i32);
        assert_eq!(String::from_utf16_lossy(&text[..len as usize]), "中");
        assert!(window.model.corner_diameter > 0);
        assert_eq!(window.styled_fonts.as_ref().unwrap().0, font_dpi);
        capture_if_requested(&window, "stage18-toast.bmp");
        let shared_text = [("中".into(), true)];
        let anchor = Some(Rect {
            left: 100,
            top: 100,
            right: 101,
            bottom: 120,
        });
        let selected_pixels = |window: &CandidateWindow| {
            let model = &window.model;
            let surface = render_surface(model).unwrap();
            let rect = model.items[0].rect;
            let step = model.width as usize;
            let radius = (model.corner_diameter as usize).div_ceil(2);
            let width = (model.items[0].runs[0].width + model.padding * 2) as usize;
            let left = rect.left as usize + radius;
            let right = rect.left as usize + width - radius;
            surface
                .bitmap
                .pixels()
                .chunks_exact(step)
                .skip(rect.top as usize + radius)
                .take((rect.bottom - rect.top) as usize - radius * 2)
                .flat_map(|row| row[left..right].iter().copied())
                .collect::<Vec<_>>()
        };
        let toast_pixels = selected_pixels(&window);
        assert!(!toast_pixels.is_empty());
        for kind in [WindowContent::Candidates(0), WindowContent::Switches] {
            window.reset();
            window.update_items(&shared_text, kind, "中", anchor);
            assert!(
                selected_pixels(&window) == toast_pixels,
                "選取文字應用相同的灰階繪製"
            );
        }
        window.show_toast("中", anchor, None);
        let mut toast_rect = Rect::default();
        assert_ne!(GetWindowRect(window.window, &mut toast_rect), 0);
        assert!(toast_rect.left >= 101 && toast_rect.bottom <= 100);
        for label in ["簡", "繁"] {
            window.show_toast(
                label,
                Some(Rect {
                    left: 100,
                    top: 100,
                    right: 101,
                    bottom: 120,
                }),
                None,
            );
            let len = GetWindowTextW(window.window, text.as_mut_ptr(), text.len() as i32);
            assert_eq!(String::from_utf16_lossy(&text[..len as usize]), label);
            assert!(window.model.corner_diameter > 0);
            assert_eq!(window.styled_fonts.as_ref().unwrap().0, font_dpi);
            let mut rect = Rect::default();
            assert_ne!(GetWindowRect(window.window, &mut rect), 0);
            assert!(rect.left >= 101 && rect.bottom <= 100);
        }
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        while IsWindowVisible(window.window) != 0 && std::time::Instant::now() < deadline {
            let mut message: Message = std::mem::zeroed();
            while PeekMessageW(&mut message, ptr::null_mut(), 0, 0, 1) != 0 {
                DispatchMessageW(&message);
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert_eq!(IsWindowVisible(window.window), 0);
        window.reset();
        assert_eq!(IsWindowVisible(window.window), 0);
        assert_ne!(DestroyWindow(window.window), 0);
        window.update(&candidates, 0, Some(edge_anchor));
        assert!(window.visible_rect().is_some(), "已銷毀的候選窗應重新建立");
        drop(window);
        assert_ne!(DestroyWindow(host), 0);
    }
}
