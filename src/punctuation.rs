//! 使用現有 RIME punctuation.yaml 的全半角與中英文標點規則。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Mapping {
    Insert(&'static str),
    Choices(&'static [&'static str]),
    Quote(&'static str, &'static str, bool),
    FullWidth(char),
}

pub(crate) fn mapping(ch: char, ascii: bool, full: bool, ascii_punct: bool) -> Option<Mapping> {
    use Mapping::{Choices, FullWidth, Insert, Quote};
    if ascii || ascii_punct {
        if full && ch == ' ' {
            return Some(Insert("　"));
        }
        return if full && ('!'..='~').contains(&ch) {
            Some(FullWidth(char::from_u32(ch as u32 + 0xfee0).unwrap()))
        } else {
            None
        };
    }
    let value = match ch {
        ' ' if full => Insert("　"),
        '0'..='9' if full => FullWidth(char::from_u32(ch as u32 + 0xfee0).unwrap()),
        ',' => Insert("，"),
        '.' => Insert("。"),
        '!' => Insert("！"),
        '?' => Insert("？"),
        ';' => Insert("；"),
        ':' => Insert("："),
        '\'' => Quote("‘", "’", false),
        '"' => Quote("“", "”", true),
        '^' => Insert("……"),
        '_' => Insert("——"),
        '(' => Insert("（"),
        ')' => Insert("）"),
        '[' => Choices(&["「", "【", "〔", "［"]),
        ']' => Choices(&["」", "】", "〕", "］"]),
        '{' => Choices(&["『", "〖", "｛"]),
        '}' => Choices(&["』", "〗", "｝"]),
        '<' => Choices(&["《", "〈", "«", "‹"]),
        '>' => Choices(&["》", "〉", "»", "›"]),
        '$' => Choices(&["￥", "$", "€", "£", "¥", "¢", "¤"]),
        '/' if full => Choices(&["／", "÷"]),
        '/' => Choices(&["、", "/", "／", "÷"]),
        '\\' if full => Choices(&["、", "＼"]),
        '\\' => Choices(&["、", "\\", "＼"]),
        '|' if full => Choices(&["·", "｜", "§", "¦"]),
        '|' => Choices(&["·", "|", "｜", "§", "¦"]),
        '@' if full => Choices(&["＠", "☯"]),
        '#' if full => Choices(&["＃", "⌘"]),
        '%' if full => Choices(&["％", "°", "℃"]),
        '%' => Choices(&["%", "％", "°", "℃"]),
        '*' if full => Choices(&["＊", "·", "・", "×", "※", "❂"]),
        '*' => Choices(&["*", "＊", "·", "・", "×", "※", "❂"]),
        '~' if !full => Choices(&["~", "～"]),
        ch if full && ('!'..='~').contains(&ch) => {
            FullWidth(char::from_u32(ch as u32 + 0xfee0).unwrap())
        }
        '@' => Insert("@"),
        '#' => Insert("#"),
        '&' => Insert("&"),
        '-' => Insert("-"),
        '+' => Insert("+"),
        '=' => Insert("="),
        '`' => Insert("`"),
        _ => return None,
    };
    Some(value)
}

#[cfg(test)]
mod tests {
    use super::{mapping, Mapping};

    #[test]
    fn uses_rime_punctuation_and_shape_settings() {
        assert_eq!(mapping('.', false, false, true), None);
        assert_eq!(
            mapping('.', false, false, false),
            Some(Mapping::Insert("。"))
        );
        assert_eq!(
            mapping('.', false, true, true),
            Some(Mapping::FullWidth('．'))
        );
        assert_eq!(
            mapping('1', false, true, false),
            Some(Mapping::FullWidth('１'))
        );
        assert_eq!(
            mapping('[', false, false, false),
            Some(Mapping::Choices(&["「", "【", "〔", "［"]))
        );
        assert_eq!(
            mapping('[', true, true, false),
            Some(Mapping::FullWidth('［'))
        );
    }
}
