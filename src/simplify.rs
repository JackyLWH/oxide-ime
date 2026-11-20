//! 依原 RIME 的 OpenCC t2s 詞表，使用最長詞優先的繁轉簡。
use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use crate::conversion::{replace_prefixes, PhraseTable};
use crate::public_data::{self, Cache};

pub(crate) struct Converter {
    characters: HashMap<String, String>,
    phrases: PhraseTable,
}

fn parse(source: &str, single: bool) -> Result<HashMap<String, String>, String> {
    let mut result = HashMap::new();
    for line in source.trim_start_matches('\u{feff}').lines() {
        if line.trim().is_empty() || line.starts_with('#') {
            continue;
        }
        let (from, values) = line.split_once('\t').ok_or("OpenCC 字詞表缺少欄位")?;
        let count = from.chars().count();
        if count == 0
            || count > 64
            || (single && count != 1)
            || from.chars().any(char::is_whitespace)
            || values.contains('\t')
            || values.split_whitespace().next().is_none()
            || line.contains('\0')
            || result.insert(from.to_owned(), values.to_owned()).is_some()
        {
            return Err("OpenCC 字詞表有無效或重複詞條".into());
        }
    }
    if result.len() < if single { 1000 } else { 100 } {
        return Err("OpenCC 字詞表不完整".into());
    }
    Ok(result)
}

pub(crate) fn current() -> Arc<Converter> {
    static EMBEDDED: OnceLock<Arc<Converter>> = OnceLock::new();
    static CACHE: Mutex<Cache<Converter, 2>> = Mutex::new(Cache::empty());
    if let Some(path) = public_data::current_directory() {
        let value = CACHE.lock().unwrap_or_else(|error| error.into_inner()).get(
            path,
            [public_data::CHARACTERS, public_data::PHRASES],
            "t2s",
            |[characters, phrases]| Converter::new(characters, phrases),
        );
        if let Some(value) = value {
            return value;
        }
    }
    EMBEDDED
        .get_or_init(|| {
            Arc::new(
                Converter::new(
                    public_data::INITIAL_CHARACTERS,
                    public_data::INITIAL_PHRASES,
                )
                .expect("內建 OpenCC 字詞表無效"),
            )
        })
        .clone()
}

impl Converter {
    pub(crate) fn new(characters: &str, phrases: &str) -> Result<Self, String> {
        let characters = parse(characters, true)?;
        let phrases = PhraseTable::new(parse(phrases, false)?);
        Ok(Self {
            characters,
            phrases,
        })
    }

    pub(crate) fn convert<'a>(&self, text: &'a str) -> Cow<'a, str> {
        replace_prefixes(text, |rest, first| {
            self.phrases
                .longest(rest, first)
                .or_else(|| {
                    let len = first.len_utf8();
                    self.characters
                        .get(&rest[..len])
                        .map(|values| (len, values.as_str()))
                })
                .and_then(|(len, values)| Some((len, values.split_whitespace().next()?)))
        })
    }
    /// Rime 優先將完整候選當作一個詞查表，保留該詞的所有轉換形式。
    pub(crate) fn candidate_forms<'a>(
        &'a self,
        text: &'a str,
    ) -> impl Iterator<Item = Cow<'a, str>> {
        let values = self
            .phrases
            .get(text)
            .or_else(|| self.characters.get(text).map(String::as_str));
        let converted = values.is_none().then(|| self.convert(text));
        values
            .into_iter()
            .flat_map(str::split_whitespace)
            .map(Cow::Borrowed)
            .chain(converted)
    }
}

#[cfg(test)]
mod tests {
    use super::{current, Converter};
    use std::{borrow::Cow, collections::HashMap};

    #[test]
    fn converts_characters_phrases_and_preserves_other_text() {
        assert_eq!(
            current().convert("頭髮與發展、老態龍鍾 😀\n繁體"),
            "头发与发展、老态龙钟 😀\n繁体"
        );
        assert_eq!(
            current().candidate_forms("钁").collect::<Vec<_>>(),
            ["镢", "䦆"]
        );
    }

    #[test]
    fn longest_phrase_wins_at_unicode_boundaries() {
        let converter = Converter {
            characters: HashMap::from([
                ("甲".into(), "一".into()),
                ("乙".into(), "二".into()),
                ("丙".into(), "三".into()),
            ]),
            phrases: super::PhraseTable::new(HashMap::from([
                ("甲乙".into(), "短".into()),
                ("甲乙丙".into(), "長".into()),
                ("🙂甲".into(), "笑".into()),
            ])),
        };
        assert_eq!(converter.convert("甲乙丙🙂甲甲乙甲"), "長笑短一");
        assert!(matches!(
            converter.convert("未知😀"),
            Cow::Borrowed("未知😀")
        ));
        assert_eq!(
            converter.convert("保留😀甲，中間乙🙂末尾"),
            "保留😀一，中間二🙂末尾"
        );
    }
}
