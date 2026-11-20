//! 共用 OpenCC 詞表的 Unicode 前綴比對。
use std::{borrow::Cow, collections::HashMap};

pub(crate) struct PhraseTable {
    entries: HashMap<String, String>,
    max_chars: HashMap<char, usize>,
}

impl PhraseTable {
    pub(crate) fn new(entries: HashMap<String, String>) -> Self {
        let mut max_chars = HashMap::<char, usize>::new();
        for key in entries.keys() {
            let mut chars = key.chars();
            if let Some(first) = chars.next() {
                let longest = max_chars.entry(first).or_default();
                *longest = (*longest).max(1 + chars.count());
            }
        }
        Self { entries, max_chars }
    }

    pub(crate) fn get(&self, key: &str) -> Option<&str> {
        self.entries.get(key).map(String::as_str)
    }

    /// 只檢查以目前字首起頭的詞長，從最長的 Unicode 前綴開始。
    pub(crate) fn longest(&self, text: &str, first: char) -> Option<(usize, &str)> {
        let &limit = self.max_chars.get(&first)?;
        let end = text
            .char_indices()
            .nth(limit)
            .map_or(text.len(), |(i, _)| i);
        text[..end].char_indices().rev().find_map(|(i, ch)| {
            let end = i + ch.len_utf8();
            self.get(&text[..end]).map(|value| (end, value))
        })
    }
}

/// 逐段替換原文；加入結果的文字不會再送回詞表。
pub(crate) fn replace_prefixes<'text, 'rule>(
    text: &'text str,
    mut find: impl FnMut(&str, char) -> Option<(usize, &'rule str)>,
) -> Cow<'text, str> {
    let mut rest = text;
    let mut output = None;
    let mut copied_until = 0;
    while let Some(ch) = rest.chars().next() {
        let len = if let Some((len, replacement)) = find(rest, ch) {
            if &rest[..len] != replacement {
                let start = text.len() - rest.len();
                let output = output.get_or_insert_with(|| String::with_capacity(text.len()));
                output.push_str(&text[copied_until..start]);
                output.push_str(replacement);
                copied_until = start + len;
            }
            len
        } else {
            ch.len_utf8()
        };
        rest = &rest[len..];
    }
    if let Some(mut output) = output {
        output.push_str(&text[copied_until..]);
        Cow::Owned(output)
    } else {
        Cow::Borrowed(text)
    }
}
