//! 模糊音設定、規則展開與反查索引。
use std::borrow::Cow;
use std::collections::{hash_map::Entry, HashMap, HashSet};
use std::sync::{Arc, OnceLock};

use fancy_regex::{Regex, RegexBuilder};
use yaml_rust2::YamlLoader;

use super::{embedded, strip_tone, Data, Reading};

struct Rule {
    pattern: Regex,
    replacement: String,
}

pub(crate) struct Fuzzy {
    rules: Vec<Rule>,
    syllables: HashMap<Arc<str>, Vec<String>>,
    pub(super) aliases: HashMap<String, Vec<Arc<Reading>>>,
    pub(super) data: Arc<Data>,
}

const DEFAULT_RULES: [&str; 7] = [
    "derive/^ng([aeiou])/$1/",
    "derive/^([aeiou])/ng$1/",
    "derive/^n(?!g)/l/",
    r"derive/n(?=\d)/ng/",
    r"derive/ng(?=\d)/n/",
    r"derive/t(?=\d)/k/",
    r"derive/k(?=\d)/t/",
];

impl Fuzzy {
    pub(crate) fn standard() -> Arc<Self> {
        static STANDARD: OnceLock<Arc<Fuzzy>> = OnceLock::new();
        STANDARD
            .get_or_init(|| Self::standard_for(embedded()))
            .clone()
    }

    #[cfg(test)]
    pub(crate) fn from_config(source: &str) -> Result<Arc<Self>, String> {
        Self::for_data(source, embedded())
    }

    pub(crate) fn for_data(source: &str, data: Arc<Data>) -> Result<Arc<Self>, String> {
        Self::parse(source, data).map(Arc::new)
    }

    pub(crate) fn standard_for(data: Arc<Data>) -> Arc<Self> {
        Arc::new(Self::from_rules(data, true, &DEFAULT_RULES).expect("內建模糊音規則無效"))
    }

    pub(crate) fn uses(&self, data: &Arc<Data>) -> bool {
        Arc::ptr_eq(&self.data, data)
    }

    fn parse(source: &str, data: Arc<Data>) -> Result<Self, String> {
        const MAX_RULES: usize = 64;
        if source.len() > 64 * 1024 {
            return Err("模糊音設定太長".into());
        }
        let docs = YamlLoader::load_from_str(source).map_err(|error| error.to_string())?;
        let [doc] = docs.as_slice() else {
            return Err("需要一份模糊音設定".into());
        };
        let enabled = doc["enabled"]
            .as_bool()
            .ok_or("enabled 需要 true 或 false")?;
        let definitions = doc["rules"].as_vec().ok_or("rules 需要規則清單")?;
        if definitions.len() > MAX_RULES {
            return Err("模糊音規則太多".into());
        }
        let rules = definitions
            .iter()
            .map(|definition| definition.as_str().ok_or("模糊音規則需要字串"))
            .collect::<Result<Vec<_>, _>>()?;
        Self::from_rules(data, enabled, &rules)
    }

    fn from_rules(data: Arc<Data>, enabled: bool, definitions: &[&str]) -> Result<Self, String> {
        let mut rules = Vec::with_capacity(definitions.len());
        for value in definitions {
            let mut fields = value.split('/');
            match (
                fields.next(),
                fields.next(),
                fields.next(),
                fields.next(),
                fields.next(),
            ) {
                (Some("derive"), Some(pattern), Some(replacement), Some(""), None)
                    if !pattern.is_empty() && pattern.len() <= 256 && replacement.len() <= 64 =>
                {
                    rules.push(Rule {
                        pattern: RegexBuilder::new(pattern)
                            .backtrack_limit(10_000)
                            .build()
                            .map_err(|err| err.to_string())?,
                        replacement: replacement.to_owned(),
                    });
                }
                _ => return Err(format!("無效模糊音規則：{value}")),
            }
        }
        if !enabled {
            rules.clear();
        }
        let mut syllables = HashMap::new();
        let mut aliases: HashMap<_, Vec<_>> = HashMap::new();
        // 反查與同音搜尋共用每個標準讀音的展開結果。
        for entry in &data.canonical {
            let reading = &entry.reading;
            let variants = match syllables.entry(reading.clone()) {
                Entry::Occupied(existing) => existing.into_mut(),
                Entry::Vacant(slot) => slot.insert(Self::derive_codes(&rules, reading)?),
            };
            for code in variants {
                aliases.entry(code.clone()).or_default().push(entry.clone());
            }
        }
        Ok(Self {
            data,
            rules,
            syllables,
            aliases,
        })
    }

    pub(super) fn codes(&self, reading: &str) -> Cow<'_, [String]> {
        if let Some(variants) = self.syllables.get(reading) {
            Cow::Borrowed(variants)
        } else {
            Cow::Owned(Self::derive_codes(&self.rules, reading).unwrap_or_default())
        }
    }

    fn derive_codes(rules: &[Rule], reading: &str) -> Result<Vec<String>, String> {
        let mut variants = vec![reading.to_owned()];
        let mut seen = HashSet::from([reading.to_owned()]);
        for rule in rules {
            for index in 0..variants.len() {
                let value = &variants[index];
                let Cow::Owned(changed) = rule
                    .pattern
                    .try_replacen(value, 0, rule.replacement.as_str())
                    .map_err(|error| error.to_string())?
                else {
                    continue;
                };
                if changed.len() <= 64
                    && changed
                        .bytes()
                        .all(|ch| ch.is_ascii_lowercase() || (b'1'..=b'6').contains(&ch))
                    && seen.insert(changed.clone())
                {
                    if variants.len() >= 128 {
                        return Err("單一粵拼的模糊變體超過 128 個".into());
                    }
                    variants.push(changed);
                }
            }
        }
        let mut seen = HashSet::new();
        Ok(variants
            .into_iter()
            .map(|value| strip_tone(&value))
            .filter(|value| !value.is_empty() && seen.insert(value.clone()))
            .collect())
    }
}
