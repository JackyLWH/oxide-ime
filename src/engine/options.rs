//! F4 設定的選項、標籤和操作。
use super::{Edit, Key, Outcome, Session};
use crate::dictionary::InputMode;

pub(super) const SWITCHES: [Switch; 6] = [
    Switch::LimitChars,
    Switch::Simplification,
    Switch::StartupMode,
    Switch::FullShape,
    Switch::AsciiPunct,
    Switch::InputMode,
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Switch {
    LimitChars,
    Simplification,
    StartupMode,
    FullShape,
    AsciiPunct,
    InputMode,
}

impl InputMode {
    fn label(self) -> &'static str {
        match self {
            Self::Cangjie => "倉頡",
            Self::Quick => "速成",
            Self::Stroke => "筆劃",
        }
    }

    pub(crate) fn toast(self) -> &'static str {
        match self {
            Self::Cangjie => "倉頡模式",
            Self::Quick => "速成模式",
            Self::Stroke => "筆劃模式",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SwitchView {
    pub name: &'static str,
    pub state: &'static str,
    pub selected: bool,
}

impl Session {
    pub(crate) fn switch_items(&self) -> Vec<SwitchView> {
        SWITCHES
            .into_iter()
            .enumerate()
            .map(|(index, option)| {
                let (name, state) = self.switch_labels(option);
                SwitchView {
                    name,
                    state,
                    selected: index == self.menu_selected,
                }
            })
            .collect()
    }

    pub(super) fn switch_labels(&self, option: Switch) -> (&'static str, &'static str) {
        match option {
            Switch::LimitChars => (
                "字集",
                if self.limit_chars {
                    "香港字"
                } else {
                    "全字"
                },
            ),
            Switch::Simplification => ("繁簡", if self.simplification { "簡" } else { "繁" }),
            Switch::StartupMode => (
                "預設輸入模式",
                if self.startup_ascii {
                    "英文"
                } else {
                    "中文"
                },
            ),
            Switch::FullShape => ("字形", if self.full_shape { "全角" } else { "半角" }),
            Switch::AsciiPunct => ("標點", if self.ascii_punct { "．，" } else { "。，" }),
            Switch::InputMode => ("輸入方案", self.input_mode.label()),
        }
    }

    pub(super) fn handle_menu(&mut self, key: Key) -> Outcome {
        match key {
            Key::Escape => {
                self.menu_open = false;
                Outcome::handled(Edit::None)
            }
            Key::Previous => {
                self.menu_selected = self.menu_selected.saturating_sub(1);
                Outcome::handled(Edit::None)
            }
            Key::Next => {
                self.menu_selected = (self.menu_selected + 1).min(SWITCHES.len() - 1);
                Outcome::handled(Edit::None)
            }
            Key::Select(index) if index < SWITCHES.len() => {
                self.menu_selected = index;
                self.toggle_option(SWITCHES[index])
            }
            Key::Space | Key::Enter => self.toggle_option(SWITCHES[self.menu_selected]),
            Key::ToggleMode => self.toggle_ascii(),
            _ => Outcome::handled(Edit::None),
        }
    }

    pub(super) fn toggle_option(&mut self, option: Switch) -> Outcome {
        match option {
            Switch::LimitChars => self.limit_chars = !self.limit_chars,
            Switch::Simplification => self.simplification = !self.simplification,
            Switch::StartupMode => self.startup_ascii = !self.startup_ascii,
            Switch::FullShape => self.full_shape = !self.full_shape,
            Switch::AsciiPunct => self.ascii_punct = !self.ascii_punct,
            Switch::InputMode => {
                self.input_mode = match self.input_mode {
                    InputMode::Cangjie => InputMode::Quick,
                    InputMode::Quick => InputMode::Stroke,
                    InputMode::Stroke => InputMode::Cangjie,
                };
                return self.cancel_composition();
            }
        }
        if self.is_composing() && matches!(option, Switch::LimitChars | Switch::Simplification) {
            self.refresh();
            Outcome::handled(Edit::Preedit(self.preedit()))
        } else {
            Outcome::handled(Edit::None)
        }
    }
}
