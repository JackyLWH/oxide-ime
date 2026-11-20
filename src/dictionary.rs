//! 倉頡、速成與筆劃的碼表索引及候選排序。
mod mode;
mod store;

use crate::commands::Command;
use crate::diagnostics::{self, Level};
use crate::public_data::{CANGJIE, CUSTOM};
use crate::simplify::Converter;
use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::{Arc, OnceLock};

pub(crate) use mode::{stroke_key, InputMode};
pub(crate) use store::{Dictionaries, DictionaryStore};

pub(crate) const DICTIONARY_FILES: [&str; 2] = [CANGJIE, CUSTOM];

#[derive(Clone, Debug, PartialEq, Eq)]
struct Entry {
    code: String,
    text: String,
    allowed_in_hong_kong: bool,
    custom: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum LookupKind {
    Cangjie,
    Quick,
    Stroke,
    Shortcut,
}

impl Entry {
    fn quick(&self) -> [u8; 2] {
        let code = self.code.as_bytes();
        [
            code[0],
            if code.len() == 1 {
                0
            } else {
                code[code.len() - 1]
            },
        ]
    }
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
    by_shortcut: Vec<usize>,
    by_quick: OnceLock<Vec<usize>>,
    max_code_len: usize,
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
        Self::parse_tables(tables, InputMode::Cangjie, &[])
    }

    pub(crate) fn parse_stroke(source: &str, cangjie: &Self) -> Result<Self, String> {
        Self::parse_tables(
            [(crate::public_data::STROKE, source)],
            InputMode::Stroke,
            cangjie.custom_entries(),
        )
    }

    fn custom_entries(&self) -> &[Entry] {
        &self.entries[self.entries.partition_point(|entry| !entry.custom)..]
    }

    fn parse_tables<'a>(
        tables: impl IntoIterator<Item = (&'a str, &'a str)>,
        mode: InputMode,
        shortcuts: &[Entry],
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
                if text.is_empty() || !mode.accepts_code(code.as_bytes()) || code.contains('*') {
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
        entries.extend_from_slice(shortcuts);
        Ok(Self::index(entries))
    }

    fn index(entries: Vec<Entry>) -> Self {
        let mut by_code: Vec<_> = (0..entries.len()).collect();
        by_code.sort_unstable_by(|&left, &right| entries[left].code.cmp(&entries[right].code));
        let by_shortcut = by_code
            .iter()
            .copied()
            .filter(|&index| entries[index].custom)
            .collect();
        let max_code_len = entries
            .iter()
            .map(|entry| entry.code.len())
            .max()
            .unwrap_or(0);
        Self {
            entries,
            by_code,
            by_shortcut,
            by_quick: OnceLock::new(),
            max_code_len,
        }
    }

    pub(crate) fn lookup(
        &self,
        mode: InputMode,
        prefix: &str,
        limit_chars: bool,
        converter: Option<&Converter>,
        ranking: &Ranking,
    ) -> Vec<DictionaryMatch> {
        if !mode.accepts_lookup(prefix.as_bytes()) || prefix.len() > self.max_code_len {
            return Vec::new();
        }
        let mut ordered = Vec::new();
        match mode {
            InputMode::Cangjie => {
                self.append_ordered(
                    &mut ordered,
                    LookupKind::Cangjie,
                    prefix,
                    limit_chars,
                    ranking,
                );
            }
            InputMode::Quick => {
                if prefix.len() <= 2 {
                    self.append_ordered(
                        &mut ordered,
                        LookupKind::Quick,
                        prefix,
                        limit_chars,
                        ranking,
                    );
                }
                self.append_ordered(
                    &mut ordered,
                    LookupKind::Shortcut,
                    prefix,
                    limit_chars,
                    ranking,
                );
            }
            InputMode::Stroke => {
                let strokes: Option<String> = prefix
                    .chars()
                    .map(|key| stroke_key(key).map(|(code, _)| code))
                    .collect();
                if let Some(strokes) = strokes {
                    self.append_ordered(
                        &mut ordered,
                        LookupKind::Stroke,
                        &strokes,
                        limit_chars,
                        ranking,
                    );
                }
                if prefix.bytes().all(|byte| byte.is_ascii_lowercase()) {
                    self.append_ordered(
                        &mut ordered,
                        LookupKind::Shortcut,
                        prefix,
                        limit_chars,
                        ranking,
                    );
                }
            }
        }
        self.render_matches(ordered, converter)
    }

    fn append_ordered(
        &self,
        ordered: &mut Vec<(bool, usize, usize)>,
        kind: LookupKind,
        prefix: &str,
        limit_chars: bool,
        ranking: &Ranking,
    ) {
        let indices = if kind == LookupKind::Quick {
            let quick = self.by_quick.get_or_init(|| {
                let mut quick: Vec<_> = (0..self.entries.len())
                    .filter(|&index| {
                        let entry = &self.entries[index];
                        !entry.custom && Command::from_marker(&entry.text).is_none()
                    })
                    .collect();
                quick.sort_unstable_by_key(|&index| self.entries[index].quick());
                quick
            });
            prefix_indices(quick, prefix.as_bytes(), |index| {
                self.entries[index].quick()
            })
        } else {
            let literal = prefix.split_once('*').map_or(prefix, |(start, _)| start);
            let index = if kind == LookupKind::Shortcut {
                &self.by_shortcut
            } else {
                &self.by_code
            };
            prefix_indices(index, literal.as_bytes(), |index| {
                self.entries[index].code.as_bytes()
            })
        };
        ordered.reserve(indices.len());
        for &index in indices {
            let entry = &self.entries[index];
            if limit_chars && !entry.allowed_in_hong_kong {
                continue;
            }
            if kind == LookupKind::Stroke && entry.custom {
                continue;
            }
            let code_len = if kind == LookupKind::Quick {
                entry.code.len().min(2)
            } else {
                entry.code.len()
            };
            if kind == LookupKind::Stroke
                && (code_len < prefix.len()
                    || !prefix
                        .bytes()
                        .zip(entry.code.bytes())
                        .all(|(input, stroke)| input == b'*' || input == stroke))
            {
                continue;
            }
            let prediction = code_len != prefix.len();
            let rank = if prediction || kind != LookupKind::Cangjie {
                ranking.rank(&entry.text)
            } else {
                0
            };
            ordered.push((prediction, rank, index));
        }
    }

    fn render_matches(
        &self,
        mut ordered: Vec<(bool, usize, usize)>,
        converter: Option<&Converter>,
    ) -> Vec<DictionaryMatch> {
        // 用原碼表序號作最後比較，並在排序後去重，確保完整匹配優先。
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

fn prefix_indices<'a, Code: AsRef<[u8]>>(
    indices: &'a [usize],
    prefix: &[u8],
    code: impl Fn(usize) -> Code,
) -> &'a [usize] {
    let compare = |index| {
        let code = code(index);
        let code = code.as_ref();
        code[..code.len().min(prefix.len())].cmp(prefix)
    };
    let start = indices.partition_point(|&index| compare(index).is_lt());
    let count = indices[start..].partition_point(|&index| compare(index).is_eq());
    &indices[start..start + count]
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
