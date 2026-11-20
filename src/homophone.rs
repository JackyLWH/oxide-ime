//! 從單一粵拼 JSON 查讀音、同音字和反查候選。
mod fuzzy;

use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, OnceLock};

use serde::de::{self, MapAccess, Visitor};
use serde::ser::SerializeMap;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::public_data::{self, Cache};

pub(crate) use fuzzy::Fuzzy;

#[derive(Clone, Copy, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
pub(crate) enum Status {
    #[serde(rename = "預設")]
    Default,
    #[serde(rename = "常用")]
    Common,
    #[serde(rename = "罕見")]
    Rare,
    #[serde(rename = "棄用")]
    Obsolete,
}

impl Status {
    pub(crate) fn from_label(label: &str) -> Option<Self> {
        match label {
            "預設" => Some(Self::Default),
            "常用" => Some(Self::Common),
            "罕見" => Some(Self::Rare),
            "棄用" => Some(Self::Obsolete),
            _ => None,
        }
    }

    fn tier(self) -> u8 {
        match self {
            Self::Default | Self::Common => 1,
            Self::Rare => 2,
            Self::Obsolete => 3,
        }
    }
}

// JSON 物件嘅欄位次序就係同級候選嘅次序。
pub(crate) struct Ordered<T>(pub Vec<(String, T)>);

impl<'de, T: Deserialize<'de>> Deserialize<'de> for Ordered<T> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Object<T>(std::marker::PhantomData<T>);
        impl<'de, T: Deserialize<'de>> Visitor<'de> for Object<T> {
            type Value = Ordered<T>;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("字庫 JSON 物件")
            }

            fn visit_map<M: MapAccess<'de>>(self, mut map: M) -> Result<Self::Value, M::Error> {
                let mut rows = Vec::new();
                let mut keys = HashSet::new();
                while let Some((key, value)) = map.next_entry::<String, T>()? {
                    if !keys.insert(key.clone()) {
                        return Err(de::Error::custom("字庫 JSON 有重複欄位"));
                    }
                    rows.push((key, value));
                }
                Ok(Ordered(rows))
            }
        }
        deserializer.deserialize_map(Object(std::marker::PhantomData))
    }
}

impl<T: Serialize> Serialize for Ordered<T> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(Some(self.0.len()))?;
        for (key, value) in &self.0 {
            map.serialize_entry(key, value)?;
        }
        map.end()
    }
}

struct Reading {
    text: Arc<str>,
    reading: Arc<str>,
    tier: u8,
    order: usize,
}

pub(crate) struct Data {
    readings: HashMap<Arc<str>, Vec<Arc<Reading>>>,
    multisyllable: Vec<Arc<Reading>>,
    canonical: Vec<Arc<Reading>>,
}

fn embedded() -> Arc<Data> {
    static DATA: OnceLock<Arc<Data>> = OnceLock::new();
    DATA.get_or_init(|| {
        Arc::new(Data::from_public_json(public_data::INITIAL_JYUTPING).expect("內建粵拼字庫無效"))
    })
    .clone()
}

pub(crate) fn current() -> Arc<Data> {
    static CACHE: Mutex<Cache<Data>> = Mutex::new(Cache::empty());
    public_data::current_directory()
        .and_then(|path| {
            CACHE.lock().unwrap_or_else(|error| error.into_inner()).get(
                path,
                [public_data::JYUTPING],
                "jyutping",
                |[source]| Data::from_json(source),
            )
        })
        .unwrap_or_else(embedded)
}

pub(crate) fn valid_reading(code: &str) -> bool {
    code.split(' ').all(|part| {
        part.as_bytes().split_last().is_some_and(|(tone, stem)| {
            (b'1'..=b'6').contains(tone)
                && !stem.is_empty()
                && stem.iter().all(u8::is_ascii_lowercase)
        })
    })
}

impl Data {
    pub(crate) fn from_public_json(source: &str) -> Result<Self, String> {
        let data = Self::from_json(source)?;
        if data.readings.len() < 20_000 {
            return Err("粵拼字庫不完整".into());
        }
        Ok(data)
    }

    fn from_json(source: &str) -> Result<Self, String> {
        let Ordered(rows): Ordered<Ordered<Status>> =
            serde_json::from_str(source.trim_start_matches('\u{feff}'))
                .map_err(|error| error.to_string())?;
        let mut data = Self {
            readings: HashMap::with_capacity(rows.len()),
            multisyllable: Vec::new(),
            canonical: Vec::new(),
        };
        let mut order = 0;
        for (text, Ordered(values)) in rows {
            if text.chars().count() != 1 || text.chars().any(char::is_control) || values.is_empty()
            {
                return Err("粵拼字庫有無效字或空白讀音".into());
            }
            let text: Arc<str> = Arc::from(text);
            let mut entries = Vec::with_capacity(values.len());
            for (reading, status) in values {
                if !valid_reading(&reading) {
                    return Err(format!("粵拼字庫有無效讀音：{text} {reading}"));
                }
                let multisyllable = reading.contains(' ');
                let entry = Arc::new(Reading {
                    text: text.clone(),
                    reading: Arc::from(reading),
                    tier: status.tier(),
                    order,
                });
                order += 1;
                entries.push(entry.clone());
                if multisyllable {
                    data.multisyllable.push(entry);
                } else {
                    data.canonical.push(entry);
                }
            }
            entries.sort_by_key(|entry| entry.tier);
            data.readings.insert(text, entries);
        }
        if data.readings.is_empty() {
            return Err("粵拼字庫沒有字音".into());
        }
        Ok(data)
    }

    fn group<'a>(
        &'a self,
        fuzzy: &'a Fuzzy,
        reading: &str,
        exact: bool,
        out: &mut Vec<&'a Reading>,
    ) {
        if reading.contains(' ') {
            let parts: Vec<_> = reading.split(' ').map(|part| fuzzy.codes(part)).collect();
            out.extend(self.multisyllable.iter().map(Arc::as_ref).filter(|entry| {
                if exact {
                    &*entry.reading == reading
                } else {
                    multisyllable_matches(fuzzy, &parts, &entry.reading)
                }
            }));
            return;
        }
        let codes = if exact {
            Cow::Owned(vec![strip_tone(reading)])
        } else {
            fuzzy.codes(reading)
        };
        out.extend(
            codes
                .iter()
                .filter_map(|code| fuzzy.aliases.get(code))
                .flatten()
                .map(Arc::as_ref)
                .filter(|entry| !exact || &*entry.reading == reading),
        );
    }
}

fn strip_tone(reading: &str) -> String {
    reading
        .chars()
        .filter(|ch| !matches!(ch, '1'..='6'))
        .collect()
}

fn multisyllable_matches(fuzzy: &Fuzzy, left: &[Cow<'_, [String]>], right: &str) -> bool {
    let mut right = right.split(' ');
    left.iter().all(|expected| {
        right.next().is_some_and(|syllable| {
            let variants = fuzzy.codes(syllable);
            expected.iter().any(|value| variants.contains(value))
        })
    }) && right.next().is_none()
}

fn ranked<'a>(entries: &'a mut [&Reading]) -> impl Iterator<Item = (String, String)> + 'a {
    entries.sort_unstable_by_key(|entry| (entry.tier, entry.order));
    let mut seen = HashSet::new();
    entries
        .iter()
        .filter(move |entry| seen.insert(&entry.text))
        .map(|entry| (entry.text.to_string(), String::new()))
}

/// 音序為零時查所有同音；其餘按來源字的讀音次序查完整音。
pub(crate) fn lookup(fuzzy: &Fuzzy, source: &str, tone_index: usize) -> Vec<(String, String)> {
    let data = &fuzzy.data;
    let readings = data.readings.get(source).map(Vec::as_slice).unwrap_or(&[]);
    if readings.is_empty() && tone_index == 0 {
        return vec![(source.to_owned(), "無拼音數據".to_owned())];
    }
    if tone_index > readings.len() {
        return vec![("「輸入錯誤」".to_owned(), String::new())];
    }
    let selected = if tone_index == 0 {
        readings
    } else {
        &readings[tone_index - 1..tone_index]
    };
    let exact = tone_index != 0;
    let label = selected
        .iter()
        .map(|entry| &*entry.reading)
        .collect::<Vec<_>>()
        .join(", ");
    let comment = format!("\"{label}\"嘅{}同音字", if exact { "完全" } else { "" });
    let mut entries = Vec::new();
    for entry in selected {
        data.group(fuzzy, &entry.reading, exact, &mut entries);
    }
    std::iter::once((source.to_owned(), comment))
        .chain(ranked(&mut entries).filter(|(text, _)| text != source))
        .collect()
}

pub(crate) fn reverse(fuzzy: &Fuzzy, code: &str, exact: bool) -> Vec<(String, String)> {
    let (stem, tone) = [
        ("vv", b'4'),
        ("xx", b'5'),
        ("qq", b'6'),
        ("v", b'1'),
        ("x", b'2'),
        ("q", b'3'),
    ]
    .into_iter()
    .find_map(|(suffix, tone)| code.strip_suffix(suffix).map(|stem| (stem, Some(tone))))
    .unwrap_or((code, None));
    let mut entries: Vec<_> = fuzzy
        .aliases
        .get(stem)
        .into_iter()
        .flatten()
        .filter_map(|entry| {
            let (last, spelling) = entry.reading.as_bytes().split_last()?;
            ((!exact || spelling == stem.as_bytes()) && tone.is_none_or(|tone| tone == *last))
                .then_some(entry.as_ref())
        })
        .collect();
    ranked(&mut entries).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lookup(source: &str, tone: usize) -> Vec<(String, String)> {
        super::lookup(&Fuzzy::standard(), source, tone)
    }

    #[test]
    fn tone_and_fuzzy_aliases_follow_current_rules() {
        let fuzzy = Fuzzy::standard();
        assert_eq!(fuzzy.codes("ngo5").as_ref(), ["ngo", "o"]);
        assert_eq!(fuzzy.codes("nei5").as_ref(), ["nei", "lei"]);
        assert_eq!(fuzzy.codes("bat1").as_ref(), ["bat", "bak"]);
        let query = [fuzzy.codes("ngaa1"), fuzzy.codes("nei5")];
        assert!(multisyllable_matches(&fuzzy, &query, "aa6 lei1"));
        assert!(!multisyllable_matches(&fuzzy, &query[..1], "ngaa1 nei5"));
    }

    #[test]
    fn shared_replacement_and_exact_reverse() {
        let ordinary = Fuzzy::standard();
        let reverse_contains = |fuzzy: &Fuzzy, code, exact, text| {
            reverse(fuzzy, code, exact)
                .iter()
                .any(|(value, _)| value == text)
        };
        assert!(reverse_contains(&ordinary, "ngo", false, "我"));
        assert!(reverse_contains(&ordinary, "ngoxx", true, "我"));
        assert!(!reverse_contains(&ordinary, "ngov", true, "我"));
        let custom = Fuzzy::from_config("enabled: true\nrules: ['derive/^ngo/xyz/']\n").unwrap();
        assert!(reverse_contains(&custom, "xyzxx", false, "我"));
        assert!(!reverse_contains(&custom, "xyzxx", true, "我"));
        assert!(super::lookup(&custom, "我", 0).len() > 1);
        let plain = Fuzzy::from_config("enabled: false\nrules: []\n").unwrap();
        assert!(!reverse_contains(&plain, "oxx", false, "我"));
    }

    #[test]
    fn lookup_keeps_source_tones_and_exact_candidates() {
        let normal = lookup("日", 0);
        assert_eq!(
            normal[0],
            ("日".into(), "\"jat6, jat2, mik6\"嘅同音字".into())
        );
        assert!(normal.iter().any(|candidate| candidate.0 == "一"));
        let exact = lookup("日", 1);
        assert_eq!(exact[0], ("日".into(), "\"jat6\"嘅完全同音字".into()));
        assert!(!exact.iter().any(|candidate| candidate.0 == "一"));
        assert_eq!(lookup("日", 4)[0].0, "「輸入錯誤」");
        assert_eq!(lookup("測試無讀音", 0)[0].1, "無拼音數據");
    }

    #[test]
    fn matches_across_tones_and_fuzzy_aliases_use_tier_then_json_order() {
        let data = Data::from_json(
            r#"{
            "罕":{"ngo5":"罕見"},
            "先":{"o5":"常用"},
            "次":{"ngo5":"預設"},
            "字":{"ngo5":"棄用","o5":"預設"},
            "末":{"ngo5":"棄用"},
            "多":{"aa1 ji1":"預設"},
            "音":{"aa6 ji4":"常用"}
        }"#,
        )
        .unwrap();
        let fuzzy = Fuzzy::standard_for(Arc::new(data));
        let texts = |results: Vec<(String, String)>| {
            results
                .into_iter()
                .map(|(text, _)| text)
                .collect::<Vec<_>>()
        };
        assert_eq!(
            texts(reverse(&fuzzy, "ngoxx", false)),
            ["先", "次", "字", "罕", "末"]
        );
        assert_eq!(
            texts(reverse(&fuzzy, "ngoxx", true)),
            ["次", "罕", "字", "末"]
        );
        assert_eq!(
            texts(super::lookup(&fuzzy, "次", 0)),
            ["次", "先", "字", "罕", "末"]
        );
        assert_eq!(texts(super::lookup(&fuzzy, "多", 0)), ["多", "音"]);
        assert!(Data::from_json(r#"{"甲":{"gaap3":"常用","gaap3":"棄用"}}"#).is_err());
    }
}
