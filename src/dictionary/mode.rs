//! 碼表種類及筆劃按鍵對應。

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(u8)]
pub(crate) enum InputMode {
    #[default]
    Cangjie,
    Quick,
    Stroke,
}

impl InputMode {
    pub(crate) fn from_id(id: u8) -> Option<Self> {
        match id {
            0 => Some(Self::Cangjie),
            1 => Some(Self::Quick),
            2 => Some(Self::Stroke),
            _ => None,
        }
    }

    pub(crate) fn accepts_code(self, code: &[u8]) -> bool {
        !code.is_empty()
            && match self {
                Self::Stroke => code.iter().all(|key| b"hspnz*".contains(key)),
                _ => code.iter().all(u8::is_ascii_lowercase),
            }
    }

    pub(crate) fn accepts_lookup(self, keys: &[u8]) -> bool {
        !keys.is_empty()
            && keys.iter().all(|key| {
                key.is_ascii_lowercase() || (self == Self::Stroke && (b'4'..=b'9').contains(key))
            })
    }
}

pub(crate) fn stroke_key(key: char) -> Option<(char, char)> {
    match key.to_ascii_lowercase() {
        'u' | '7' => Some(('h', '一')),
        'i' | '8' => Some(('s', '丨')),
        'o' | '9' => Some(('p', '丿')),
        'j' | '4' => Some(('n', '丶')),
        'k' | '5' => Some(('z', '乛')),
        'l' | '6' => Some(('*', '＊')),
        _ => None,
    }
}
