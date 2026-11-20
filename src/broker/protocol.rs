//! RI01 的長度欄位、文字編碼與封包上限。
use std::time::Duration;

pub(super) const MAX_PREFIX: usize = 256;
pub(super) const MAX_BODY: usize = 64 * 1024 * 1024;
// 筆劃單一通配碼可涵蓋十萬以上的異體與延伸區字。
pub(super) const MAX_COUNT: usize = 250_000;
pub(super) const LOOKUP_MODE_SHIFT: u8 = 2;
pub(super) const RAW_STROKE_KEYS: u8 = 0x10;
pub(super) const EXTENDED_MODES: u8 = 0x80;
pub(super) const TIMEOUT: Duration = Duration::from_millis(750);
pub(super) const MAGIC: &[u8; 4] = b"RI01";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub(super) enum Operation {
    LegacyLookup = 0,
    Resolve = 1,
    PermissionStatus = 2,
    Reverse = 3,
    Lookup = 4,
    ConvertClipboard = 5,
    ReverseMatch = 6,
    Jyutping = 8,
    Homophones = 9,
    Modes = 10,
}

impl TryFrom<u8> for Operation {
    type Error = String;

    fn try_from(value: u8) -> Result<Self, String> {
        let operation = match value {
            0 => Self::LegacyLookup,
            1 => Self::Resolve,
            2 => Self::PermissionStatus,
            3 => Self::Reverse,
            4 => Self::Lookup,
            5 => Self::ConvertClipboard,
            6 => Self::ReverseMatch,
            8 => Self::Jyutping,
            9 => Self::Homophones,
            10 => Self::Modes,
            _ => return Err("查字要求無效".into()),
        };
        Ok(operation)
    }
}

pub(super) fn read_u32(bytes: &[u8]) -> u32 {
    u32::from_le_bytes(bytes.try_into().expect("四位元組"))
}

fn take_text<'a>(body: &mut &'a [u8]) -> Result<&'a str, String> {
    let (length, rest) = body.split_at_checked(4).ok_or("候選資料不完整")?;
    let (text, rest) = rest
        .split_at_checked(read_u32(length) as usize)
        .ok_or("候選資料不完整")?;
    *body = rest;
    std::str::from_utf8(text).map_err(|_| "候選文字編碼無效".into())
}

pub(super) fn decode_response(mut body: &[u8], count: usize) -> Result<Vec<&str>, String> {
    if count > MAX_COUNT || count > body.len() / 4 {
        return Err("候選資料數量無效".into());
    }
    let mut entries = Vec::with_capacity(count);
    for _ in 0..count {
        entries.push(take_text(&mut body)?);
    }
    if !body.is_empty() {
        return Err("候選資料有多餘內容".into());
    }
    Ok(entries)
}

pub(super) fn decode_single(
    mut body: &[u8],
    count: usize,
    invalid: &str,
) -> Result<String, String> {
    if count != 1 {
        return Err(invalid.to_owned());
    }
    let text = take_text(&mut body)?;
    if !body.is_empty() {
        return Err("候選資料有多餘內容".into());
    }
    Ok(text.to_owned())
}

pub(super) fn add_text(body: &mut Vec<u8>, text: &str) -> Result<(), String> {
    if body
        .len()
        .checked_add(4)
        .and_then(|size| size.checked_add(text.len()))
        .is_none_or(|size| size > MAX_BODY)
    {
        return Err("候選資料過長".into());
    }
    body.extend_from_slice(&(text.len() as u32).to_le_bytes());
    body.extend_from_slice(text.as_bytes());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn candidate_protocol_rejects_truncated_or_oversized_data() {
        let mut body = Vec::new();
        body.extend_from_slice(&3u32.to_le_bytes());
        body.extend_from_slice("日".as_bytes());
        assert_eq!(decode_response(&body, 1).unwrap(), ["日"]);
        assert_eq!(decode_single(&body, 1, "無效").unwrap(), "日");
        assert!(decode_response(&body[..6], 1).is_err());
        assert!(decode_response(&body, 2).is_err());
        assert!(decode_response(&body, 0).is_err());
        assert!(decode_response(&body, MAX_COUNT + 1).is_err());
    }
}
