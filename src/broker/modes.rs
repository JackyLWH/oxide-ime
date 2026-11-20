//! 每個登入工作階段的即時模式，以及下次登入使用的預設值。
use crate::{
    project_files,
    win32::registry::{Key, HKCU},
};
use std::path::{Path, PathBuf};

use super::pipe::{current_logon_sid, Raw};

const ASCII: u32 = 1;
const SIMPLIFIED: u32 = 2;
const STARTUP_ASCII: u32 = 4;
const JYUTPING: u32 = 8;
const EXACT: u32 = 16;
const LIVE: u32 = ASCII | SIMPLIFIED | JYUTPING | EXACT;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Modes {
    pub ascii: bool,
    pub simplified: bool,
    pub startup_ascii: bool,
    pub reverse_exact: Option<bool>,
}

impl Modes {
    pub(super) fn from_bits(bits: u32) -> Self {
        Self {
            ascii: bits & ASCII != 0,
            simplified: bits & SIMPLIFIED != 0,
            startup_ascii: bits & STARTUP_ASCII != 0,
            reverse_exact: (bits & (ASCII | JYUTPING) == JYUTPING).then_some(bits & EXACT != 0),
        }
    }

    pub(super) fn bits(self) -> u32 {
        (u32::from(self.ascii) * ASCII)
            | (u32::from(self.simplified) * SIMPLIFIED)
            | (u32::from(self.startup_ascii) * STARTUP_ASCII)
            | self
                .reverse_exact
                .map_or(0, |exact| JYUTPING | (u32::from(exact) * EXACT))
    }
}

#[derive(Clone, Copy)]
#[repr(u8)]
pub(crate) enum ModeAction {
    Read = 0,
    ToggleAscii = 1,
    ToggleSimplification = 2,
    ToggleStartup = 3,
    SetCangjie = 4,
    SetJyutping = 5,
    SetJyutpingExact = 6,
}

impl ModeAction {
    pub(crate) fn for_reverse_mode(exact: Option<bool>) -> Self {
        match exact {
            None => Self::SetCangjie,
            Some(false) => Self::SetJyutping,
            Some(true) => Self::SetJyutpingExact,
        }
    }

    fn update_live(self, current: u32) -> u32 {
        let reverse = JYUTPING | EXACT;
        match self {
            Self::ToggleAscii => (current ^ ASCII) & !reverse,
            Self::ToggleSimplification => current ^ SIMPLIFIED,
            Self::SetCangjie => current & !reverse,
            Self::SetJyutping if current & ASCII == 0 => (current & !reverse) | JYUTPING,
            Self::SetJyutpingExact if current & ASCII == 0 => current | reverse,
            _ => current,
        }
    }
}

impl TryFrom<u8> for ModeAction {
    type Error = String;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(Self::Read),
            1 => Ok(Self::ToggleAscii),
            2 => Ok(Self::ToggleSimplification),
            3 => Ok(Self::ToggleStartup),
            4 => Ok(Self::SetCangjie),
            5 => Ok(Self::SetJyutping),
            6 => Ok(Self::SetJyutpingExact),
            _ => Err("輸入模式操作無效".into()),
        }
    }
}

fn mode_key(parent: Raw, path: &str, volatile: bool) -> Result<Key, String> {
    // QUERY_VALUE | SET_VALUE | CREATE_SUB_KEY | WOW64_64KEY
    unsafe { Key::create(parent, path, u32::from(volatile), 0x0007 | 0x0100) }
        .map(|(key, _)| key)
        .map_err(|rc| format!("無法開啟輸入設定（{rc}）"))
}

pub(super) struct ModeStore {
    settings: Key,
    startup_file: Option<PathBuf>,
    startup: u32,
    login: Key,
    live: u32,
}

impl ModeStore {
    pub(super) fn new(project_id: u64, root: &Path) -> Result<Self, String> {
        let installed = project_files::installed(root);
        let startup_file = project_files::user_data(root)?.join("setting/startup-mode.txt");
        let startup_file = (installed || startup_file.is_file()).then_some(startup_file);
        let namespace = if installed {
            r"Software\OxideIME\Runtime"
        } else {
            r"Software\RustIme"
        };
        let settings = mode_key(HKCU, &format!(r"{namespace}\{project_id:016x}"), false)?;
        let sessions = mode_key(settings.0, "Logons", true)?;
        let login = mode_key(sessions.0, &current_logon_sid()?, true)?;
        let startup = Self::startup(&settings, startup_file.as_deref());
        let live = match login.dword("Modes") {
            Some(live) => live & LIVE,
            None => {
                login.set_dword("Modes", startup)?;
                startup
            }
        };
        Ok(Self {
            settings,
            startup_file,
            startup,
            login,
            live,
        })
    }

    pub(super) fn apply(&mut self, action: ModeAction) -> Result<Modes, String> {
        if matches!(action, ModeAction::ToggleStartup) {
            let startup = Self::startup(&self.settings, self.startup_file.as_deref()) ^ 1;
            if let Some(path) = &self.startup_file {
                let contents = if startup == 1 { b"en\n" } else { b"zh\n" };
                project_files::replace_file(path, contents)
                    .map_err(|error| format!("無法儲存預設輸入模式（{error}）"))?;
            } else {
                self.settings.set_dword("StartupAscii", startup)?;
            }
            self.startup = startup;
        }
        let live = action.update_live(self.live);
        if live != self.live {
            self.login.set_dword("Modes", live)?;
            self.live = live;
        }
        Ok(Modes::from_bits(live | (self.startup * STARTUP_ASCII)))
    }

    fn startup(settings: &Key, file: Option<&Path>) -> u32 {
        match file {
            Some(path) => {
                u32::from(!std::fs::read_to_string(path).is_ok_and(|value| value.trim() == "zh"))
            }
            None => settings.dword("StartupAscii").unwrap_or(1) & 1,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jyutping_round_trips_in_shared_modes_until_commit_or_english() {
        let exact = ModeAction::SetJyutpingExact.update_live(SIMPLIFIED);
        assert_eq!(Modes::from_bits(exact).reverse_exact, Some(true));
        let fuzzy = ModeAction::SetJyutping.update_live(exact);
        assert_eq!(
            Modes::from_bits(fuzzy | STARTUP_ASCII).bits(),
            fuzzy | STARTUP_ASCII
        );
        assert_eq!(Modes::from_bits(fuzzy).reverse_exact, Some(false));
        assert_eq!(ModeAction::SetCangjie.update_live(fuzzy), SIMPLIFIED);
        let english = ModeAction::ToggleAscii.update_live(exact);
        assert_eq!(english, ASCII | SIMPLIFIED);
        assert_eq!(ModeAction::SetJyutping.update_live(english), english);
    }
}
