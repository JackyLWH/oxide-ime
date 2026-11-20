//! 將 Windows 按鍵轉成輸入引擎辨識的操作。
use super::ffi;
use crate::engine::{Key, Switch};

#[derive(Clone, Copy, Default)]
pub(super) struct Modifiers {
    shift: bool,
    ctrl: bool,
    alt: bool,
    win: bool,
    caps: bool,
}

impl Modifiers {
    pub(super) fn can_toggle_shift(self) -> bool {
        !self.ctrl && !self.alt && !self.win
    }
}

pub(super) fn modifiers() -> Modifiers {
    unsafe {
        Modifiers {
            shift: ffi::GetKeyState(0x10) < 0,
            ctrl: ffi::GetKeyState(0x11) < 0,
            alt: ffi::GetKeyState(0x12) < 0,
            win: ffi::GetKeyState(0x5b) < 0 || ffi::GetKeyState(0x5c) < 0,
            caps: ffi::GetKeyState(0x14) & 1 != 0,
        }
    }
}

pub(super) fn shift_key(key: usize) -> bool {
    matches!(key, 0x10 | 0xa0 | 0xa1)
}

fn letter_for_key(key: usize, uppercase: bool) -> Option<Key> {
    (0x41..=0x5a).contains(&key).then(|| {
        let letter = key as u8 as char;
        Key::Letter(if uppercase {
            letter
        } else {
            letter.to_ascii_lowercase()
        })
    })
}

fn symbol_for_key(key: usize, shift: bool) -> Option<char> {
    let (plain, shifted) = match key {
        0x30..=0x39 => {
            let index = key - 0x30;
            return Some(if shift {
                b")!@#$%^&*("[index]
            } else {
                b'0' + index as u8
            } as char);
        }
        0x60..=0x69 => return Some((b'0' + (key - 0x60) as u8) as char),
        0xba => (';', ':'),
        0xbb => ('=', '+'),
        0xbc => (',', '<'),
        0xbd => ('-', '_'),
        0xbe => ('.', '>'),
        0xbf => ('/', '?'),
        0xc0 => ('`', '~'),
        0xdb => ('[', '{'),
        0xdc => ('\\', '|'),
        0xdd => (']', '}'),
        0xde => ('\'', '"'),
        0x6a => ('*', '*'),
        0x6b => ('+', '+'),
        0x6d => ('-', '-'),
        0x6e => ('.', '.'),
        0x6f => ('/', '/'),
        _ => return None,
    };
    Some(if shift { shifted } else { plain })
}

pub(super) fn translate_key(
    key: usize,
    modifiers: Modifiers,
    composing: bool,
    menu_open: bool,
    stroke_input: bool,
) -> Option<Key> {
    let Modifiers {
        shift,
        ctrl,
        alt,
        win,
        caps,
    } = modifiers;
    // 繁簡快捷鍵在組字中也要先於其他組字按鍵限制處理。
    if ctrl && !shift && !alt && !win && key == 0xbf {
        return Some(Key::ToggleSimplification);
    }
    if !shift && !alt && !win && key == 0xc0 {
        if ctrl {
            return Some(Key::ToggleReverseExact);
        }
        if !menu_open {
            return Some(Key::ToggleJyutping);
        }
    }
    if !ctrl && !shift && !alt && !win && key == 0x73 && !composing {
        return Some(Key::Menu);
    }
    if stroke_input && !menu_open && !ctrl && !shift && !alt && !win && (0x64..=0x69).contains(&key)
    {
        return Some(Key::Numpad((b'0' + (key - 0x60) as u8) as char));
    }
    if composing {
        // Caps Lock 必須交給系統切換狀態；單獨 Shift 由 key sink 處理。
        if key == 0x14 || shift_key(key) {
            return None;
        }
        if ctrl || alt || win {
            return Some(Key::Block);
        }
        if let Some(letter) = letter_for_key(key, shift ^ caps) {
            return Some(letter);
        }
        if shift {
            return Some(Key::Block);
        }
        return Some(match key {
            0x08 => Key::Backspace,
            0x1b => Key::Escape,
            0x0d => Key::Enter,
            0x20 => Key::Space,
            0x31..=0x39 => Key::Select(key - 0x31),
            0x25 | 0x26 => Key::Previous,
            0x27 | 0x28 => Key::Next,
            _ => Key::Block,
        });
    }
    if ctrl && shift && !alt && !win && key == 0x32 {
        return Some(Key::ToggleMode);
    }
    if ctrl && shift && !alt && !win && key == 0x33 {
        return Some(Key::ToggleOption(Switch::FullShape));
    }
    if ctrl && shift && !alt && !win && key == 0x34 {
        return Some(Key::ToggleOption(Switch::Simplification));
    }
    if win {
        return None;
    }
    if menu_open {
        if ctrl || alt || shift {
            return None;
        }
        return Some(match key {
            0x31..=0x39 => Key::Select(key - 0x31),
            0x61..=0x69 => Key::Select(key - 0x61),
            0x1b => Key::Escape,
            0x0d => Key::Enter,
            0x20 => Key::Space,
            0x21 | 0x25 | 0x26 => Key::Previous,
            0x22 | 0x27 | 0x28 => Key::Next,
            _ => Key::Block,
        });
    }
    if ctrl || alt {
        return None;
    }
    if key == 0x20 {
        Some(Key::Space)
    } else {
        letter_for_key(key, shift ^ caps).or_else(|| symbol_for_key(key, shift).map(Key::Symbol))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn translate_key(
        key: usize,
        modifiers: Modifiers,
        composing: bool,
        menu_open: bool,
    ) -> Option<Key> {
        super::translate_key(key, modifiers, composing, menu_open, false)
    }

    #[test]
    fn control_slash_toggles_simplification_during_and_outside_composition() {
        let control = Modifiers {
            ctrl: true,
            ..Modifiers::default()
        };
        for (composing, menu_open) in [(false, false), (true, false), (false, true)] {
            assert_eq!(
                translate_key(0xbf, control, composing, menu_open),
                Some(Key::ToggleSimplification)
            );
        }
        assert_eq!(
            translate_key(
                0xbf,
                Modifiers {
                    alt: true,
                    ..control
                },
                true,
                false
            ),
            Some(Key::Block)
        );
        assert_eq!(
            translate_key(
                0x33,
                Modifiers {
                    shift: true,
                    ..control
                },
                false,
                false
            ),
            Some(Key::ToggleOption(Switch::FullShape))
        );
        assert_eq!(
            translate_key(
                0x34,
                Modifiers {
                    shift: true,
                    ..control
                },
                false,
                false
            ),
            Some(Key::ToggleOption(Switch::Simplification))
        );
    }

    #[test]
    fn composing_keys_are_limited_without_changing_inactive_shortcuts() {
        assert_eq!(
            translate_key(0x41, Modifiers::default(), false, false),
            Some(Key::Letter('a'))
        );
        assert_eq!(
            translate_key(
                0x41,
                Modifiers {
                    shift: true,
                    ..Modifiers::default()
                },
                false,
                false
            ),
            Some(Key::Letter('A'))
        );
        assert_eq!(
            translate_key(
                0x32,
                Modifiers {
                    shift: true,
                    ctrl: true,
                    ..Modifiers::default()
                },
                false,
                false
            ),
            Some(Key::ToggleMode)
        );
        assert_eq!(
            translate_key(
                0x56,
                Modifiers {
                    ctrl: true,
                    ..Modifiers::default()
                },
                true,
                false
            ),
            Some(Key::Block)
        );
        assert_eq!(
            translate_key(0x1b, Modifiers::default(), true, false),
            Some(Key::Escape)
        );
        assert_eq!(
            translate_key(0x31, Modifiers::default(), true, false),
            Some(Key::Select(0))
        );
        assert_eq!(
            translate_key(0x70, Modifiers::default(), false, false),
            None
        );
        assert_eq!(
            translate_key(0x73, Modifiers::default(), false, false),
            Some(Key::Menu)
        );
        assert_eq!(
            translate_key(0x35, Modifiers::default(), false, true),
            Some(Key::Select(4))
        );
        assert_eq!(
            translate_key(0x36, Modifiers::default(), false, true),
            Some(Key::Select(5))
        );
        assert_eq!(
            translate_key(0xbf, Modifiers::default(), false, false),
            Some(Key::Symbol('/'))
        );
        for key in [
            0x09, 0x21, 0x22, 0x30, 0x60, 0x61, 0x69, 0x6a, 0x6b, 0x6d, 0x6e, 0x6f, 0x73, 0xbe,
            0xbf,
        ] {
            assert_eq!(
                translate_key(key, Modifiers::default(), true, false),
                Some(Key::Block),
                "{key:#x}"
            );
        }
        assert_eq!(translate_key(0x14, Modifiers::default(), true, false), None);
        assert_eq!(
            translate_key(0x08, Modifiers::default(), true, false),
            Some(Key::Backspace)
        );
        assert_eq!(
            translate_key(0x28, Modifiers::default(), true, false),
            Some(Key::Next)
        );
    }

    #[test]
    fn stroke_numpad_keys_are_distinct_from_selection_and_navigation() {
        for key in 0x64..=0x69 {
            let digit = (b'0' + (key - 0x60) as u8) as char;
            for composing in [false, true] {
                assert_eq!(
                    super::translate_key(key, Modifiers::default(), composing, false, true),
                    Some(Key::Numpad(digit))
                );
            }
            assert_eq!(
                super::translate_key(key - 0x30, Modifiers::default(), true, false, true),
                Some(Key::Select(key - 0x61))
            );
        }
        assert_eq!(
            super::translate_key(0x25, Modifiers::default(), true, false, true),
            Some(Key::Previous)
        );
    }
}
