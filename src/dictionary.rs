//! 倉頡碼表的解析、排序及候選文字。
mod store;

use crate::commands::Command;
use crate::diagnostics::{self, Level};
use crate::public_data::{CANGJIE, CUSTOM};
use crate::simplify::Converter;
use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::{Arc, OnceLock};

pub(crate) use store::{Dictionaries, DictionaryStore};

pub(crate) const DICTIONARY_FILES: [&str; 2] = [CANGJIE, CUSTOM];

#[derive(Debug)]
struct Entry {
    code: String,
    text: String,
    allowed_in_hong_kong: bool,
    custom: bool,
}

#[derive(Default)]
pub(crate) struct Ranking(HashMap<char, usize>);

impl Ranking {
    pub(crate) fn parse(source: &str) -> Result<Self, String> {
        let mut ranks = HashMap::new();
        for (rank, line) in source.trim_start_matches('\u{feff}').lines().enumerate() {
            let mut chars = line.chars();
            let ch = chars.next().ok_or("排名有空白列")?;
            if chars.next().is_some() || ch.is_control() || ranks.insert(ch, rank).is_some() {
                return Err("排名有無效或重複的字".into());
            }
        }
        if ranks.is_empty() {
            return Err("排名沒有字條".into());
        }
        Ok(Self(ranks))
    }

    pub(crate) fn len(&self) -> usize {
        self.0.len()
    }

    pub(crate) fn rank(&self, text: &str) -> usize {
        let mut chars = text.chars();
        match (chars.next(), chars.next()) {
            (Some(ch), None) => self.0.get(&ch).copied().unwrap_or(usize::MAX),
            _ => usize::MAX,
        }
    }

    fn standard() -> Arc<Self> {
        static STANDARD: OnceLock<Arc<Ranking>> = OnceLock::new();
        STANDARD
            .get_or_init(|| {
                Arc::new(Self::parse(crate::public_data::INITIAL_RANKING).expect("內建排名無效"))
            })
            .clone()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct DictionaryMatch {
    entry: usize,
    form: usize,
}

#[derive(Debug)]
pub(crate) struct Dictionary {
    entries: Vec<Entry>,
    by_code: Vec<usize>,
}

impl Dictionary {
    pub(crate) fn load(base: &Path, custom: Option<&Path>) -> Result<Self, String> {
        let mut tables = Vec::new();
        for (table, path) in std::iter::once(base).chain(custom).enumerate() {
            let content = std::fs::read_to_string(path).map_err(|err| {
                diagnostics::event(
                    Level::Warning,
                    "dictionary",
                    "read_error",
                    &[
                        ("table", table as i64),
                        ("win32", i64::from(err.raw_os_error().unwrap_or(0))),
                    ],
                );
                format!("無法讀取 {}：{err}", path.display())
            })?;
            tables.push((path.to_string_lossy(), content));
        }
        Self::parse(
            tables
                .iter()
                .map(|(name, text)| (name.as_ref(), text.as_str())),
        )
    }

    pub(crate) fn parse<'a>(
        tables: impl IntoIterator<Item = (&'a str, &'a str)>,
    ) -> Result<Self, String> {
        let mut entries = Vec::new();
        for (table, (name, source)) in tables.into_iter().enumerate() {
            let mut in_body = false;
            for (line_number, line) in source.lines().enumerate() {
                if !in_body {
                    if line.trim_start_matches('\u{feff}') == "..." {
                        in_body = true;
                    }
                    continue;
                }
                if line.is_empty() || line.starts_with('#') {
                    continue;
                }
                let Some((text, code)) = line.split_once('\t') else {
                    invalid_row(table, line_number + 1, 1);
                    return Err(format!("{name} 第 {} 行缺少編碼欄位", line_number + 1));
                };
                if text.is_empty()
                    || code.is_empty()
                    || !code.bytes().all(|b| b.is_ascii_lowercase())
                {
                    invalid_row(table, line_number + 1, 2);
                    return Err(format!("{name} 第 {} 行的詞條或編碼無效", line_number + 1));
                }
                entries.push(Entry {
                    code: code.to_owned(),
                    text: text.to_owned(),
                    allowed_in_hong_kong: allowed_text(text),
                    custom: table > 0,
                });
            }
            if !in_body {
                invalid_row(table, 0, 3);
                return Err(format!("{name} 缺少字庫表頭結束標記"));
            }
        }
        Ok(Self::index(entries))
    }

    fn index(entries: Vec<Entry>) -> Self {
        let mut by_code: Vec<_> = (0..entries.len()).collect();
        by_code.sort_unstable_by(|&left, &right| entries[left].code.cmp(&entries[right].code));
        Self { entries, by_code }
    }

    pub(crate) fn lookup(
        &self,
        prefix: &str,
        limit_chars: bool,
        converter: Option<&Converter>,
        ranking: &Ranking,
    ) -> Vec<DictionaryMatch> {
        if prefix.is_empty() || !prefix.bytes().all(|byte| byte.is_ascii_lowercase()) {
            return Vec::new();
        }
        let start = self
            .by_code
            .partition_point(|&index| self.entries[index].code.as_str() < prefix);
        let mut ordered: Vec<_> = self.by_code[start..]
            .iter()
            .copied()
            .take_while(|&index| self.entries[index].code.starts_with(prefix))
            .filter_map(|index| {
                let entry = &self.entries[index];
                if limit_chars && !entry.allowed_in_hong_kong {
                    return None;
                }
                let completion = entry.code != prefix;
                let rank = if completion {
                    ranking.rank(&entry.text)
                } else {
                    0
                };
                Some((completion, rank, index))
            })
            .collect();
        // 完整匹配和同排名的候選依原碼表序號排列。
        ordered.sort_unstable();
        let mut seen = HashSet::new();
        let mut result = Vec::new();
        for (_, _, index) in ordered {
            let entry = &self.entries[index];
            if let Some(converter) = converter {
                for (form, text) in converter.candidate_forms(&entry.text).enumerate() {
                    if seen.insert(text) {
                        result.push(DictionaryMatch { entry: index, form });
                    }
                }
            } else if seen.insert(Cow::Borrowed(entry.text.as_str())) {
                result.push(DictionaryMatch {
                    entry: index,
                    form: 0,
                });
            }
        }
        result
    }

    pub(crate) fn text<'a>(
        &'a self,
        item: DictionaryMatch,
        converter: Option<&'a Converter>,
    ) -> Cow<'a, str> {
        let text = self.entries[item.entry].text.as_str();
        let rendered = if let Some(converter) = converter {
            converter
                .candidate_forms(text)
                .nth(item.form)
                .expect("候選轉換索引無效")
        } else {
            Cow::Borrowed(text)
        };
        decode_dictionary_text(rendered)
    }

    pub(crate) fn original(&self, item: DictionaryMatch) -> Cow<'_, str> {
        decode_dictionary_text(Cow::Borrowed(&self.entries[item.entry].text))
    }

    pub(crate) fn command(&self, item: DictionaryMatch) -> Option<Command> {
        Command::from_marker(&self.entries[item.entry].text)
    }

    pub(crate) fn is_custom(&self, item: DictionaryMatch) -> bool {
        self.entries[item.entry].custom
    }

    pub(crate) fn reverse(&self, text: &str) -> String {
        let mut seen = HashSet::new();
        self.entries
            .iter()
            .filter(|entry| {
                entry.text == text
                    || (entry.text.contains("\\n")
                        && decode_dictionary_text(Cow::Borrowed(&entry.text)) == text)
            })
            .filter_map(|entry| {
                seen.insert(entry.code.as_str())
                    .then_some(entry.code.as_str())
            })
            .collect::<Vec<_>>()
            .join(" ")
    }
}

fn invalid_row(table: usize, line: usize, reason: i64) {
    diagnostics::event(
        Level::Warning,
        "dictionary",
        "invalid_table",
        &[
            ("table", table as i64),
            ("line", line as i64),
            ("reason", reason),
        ],
    );
}

/// 與現有 `limit_chars` 預設值相同：保留 BMP 與指定 emoji 範圍。
pub(crate) fn allowed_text(text: &str) -> bool {
    !text.is_empty()
        && text
            .chars()
            .all(|ch| (ch as u32) <= 0xffff || (0x1f000..=0x1ffff).contains(&(ch as u32)))
}

pub(crate) fn decode_dictionary_text(text: Cow<'_, str>) -> Cow<'_, str> {
    if !text.contains("\\n") {
        return text;
    }
    let mut chars = text.chars().peekable();
    let mut result = String::with_capacity(text.len());
    while let Some(ch) = chars.next() {
        if ch == '\\' {
            match chars.peek().copied() {
                Some('n') => {
                    chars.next();
                    result.push('\n');
                }
                Some('\\') => {
                    chars.next();
                    result.push('\\');
                }
                _ => result.push(ch),
            }
        } else {
            result.push(ch);
        }
    }
    Cow::Owned(result)
}
