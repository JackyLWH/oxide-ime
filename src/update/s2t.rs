//! 將 OpenCC 簡轉繁詞組按同文堂的香港用字規則重建為 JSON。
use std::collections::{BTreeMap, HashSet};

#[cfg(any(test, feature = "private-discord"))]
use serde::{Deserialize, Serialize};

use super::message;

const TAI_EXCEPTIONS: [&str; 6] = ["臺灣", "臺北", "臺中", "臺南", "臺東", "臺西"];

#[cfg(any(test, feature = "private-discord"))]
#[derive(Deserialize, PartialEq, Eq, Serialize)]
pub(super) struct Choice {
    candidates: [String; 2],
    pub(super) output: String,
}

#[cfg(any(test, feature = "private-discord"))]
pub(super) type Choices = BTreeMap<String, Choice>;

struct Resolved {
    phrases: BTreeMap<String, String>,
    #[cfg(any(test, feature = "private-discord"))]
    choices: Choices,
}

pub(super) struct Build {
    pub(super) json: String,
    #[cfg(feature = "private-discord")]
    pub(super) choices: Choices,
}

struct Rules {
    overrides: BTreeMap<String, String>,
    replacements: Vec<(String, String)>,
    exclusions: HashSet<String>,
}

impl Rules {
    fn parse([overrides, replacements, exclusions]: [&str; 3]) -> Result<Self, String> {
        Ok(Self {
            overrides: serde_json::from_str(overrides).map_err(message)?,
            replacements: serde_json::from_str(replacements).map_err(message)?,
            exclusions: serde_json::from_str(exclusions).map_err(message)?,
        })
    }

    fn apply(&self, source: &str) -> Result<Resolved, String> {
        let mut seen = HashSet::new();
        let mut phrases = BTreeMap::new();
        #[cfg(any(test, feature = "private-discord"))]
        let mut choices = Choices::new();
        for line in source.trim_start_matches('\u{feff}').lines().map(str::trim) {
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let (key, values) = line.split_once('\t').ok_or("OpenCC 簡轉繁詞組缺少欄位")?;
            let mut values = values.split_whitespace();
            let first = values.next().ok_or("OpenCC 簡轉繁詞組缺少候選")?;
            if key.is_empty()
                || key.chars().any(char::is_whitespace)
                || line.contains('\0')
                || !seen.insert(key)
            {
                return Err(format!("OpenCC 簡轉繁有無效或重複詞條（{key}）"));
            }
            if self.exclusions.contains(key) {
                continue;
            }
            let (mut value, alternative) = match (self.overrides.get(key), values.next()) {
                (Some(value), _) => (value.clone(), None),
                (None, None) => (first.to_owned(), None),
                (None, Some(second)) if values.next().is_none() => (first.to_owned(), Some(second)),
                (None, Some(_)) => {
                    return Err(format!("同文堂規則需要為 OpenCC 詞組「{key}」指定單一候選"));
                }
            };
            for (old, new) in &self.replacements {
                value = value.replace(old, new);
            }
            if !TAI_EXCEPTIONS.iter().any(|part| value.contains(part)) {
                value = value.replace('檯', "枱").replace('臺', "台");
            }
            #[cfg(not(any(test, feature = "private-discord")))]
            let _ = alternative;
            #[cfg(any(test, feature = "private-discord"))]
            if let Some(second) = alternative {
                choices.insert(
                    key.to_owned(),
                    Choice {
                        candidates: [first.to_owned(), second.to_owned()],
                        output: value.clone(),
                    },
                );
            }
            // 相同的原文及結果仍可保護完整詞組，避免匹配較短的詞。
            phrases.insert(key.to_owned(), value);
        }
        for key in self.overrides.keys().chain(&self.exclusions) {
            if !seen.contains(key.as_str()) {
                return Err(format!("同文堂規則的詞組「{key}」已從 OpenCC 上游移除"));
            }
        }
        Ok(Resolved {
            phrases,
            #[cfg(any(test, feature = "private-discord"))]
            choices,
        })
    }
}

pub(super) fn build(source: &str, rules: [&str; 3]) -> Result<Build, String> {
    let resolved = Rules::parse(rules)?.apply(source)?;
    if resolved.phrases.len() < 40_000 {
        return Err("OpenCC 簡轉繁詞組表不完整".into());
    }
    let json = serde_json::to_string(&resolved.phrases).map_err(message)?;
    Ok(Build {
        json,
        #[cfg(feature = "private-discord")]
        choices: resolved.choices,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uses_ordered_replacements_and_keeps_protection_words() {
        let rules = Rules {
            overrides: BTreeMap::from([("升平".into(), "昇平".into())]),
            replacements: vec![("乾".into(), "干".into()), ("干".into(), "幹".into())],
            exclusions: HashSet::from(["换发".into()]),
        };
        let source =
            "升平\t升平 昇平\n换发\t換發 換髮\n出外\t出外\n干燥\t乾燥 干燥\n台北\t臺北\n台面\t檯面\n";
        let Resolved {
            phrases: result,
            choices,
        } = rules.apply(source).unwrap();
        assert_eq!(result["升平"], "昇平");
        assert!(!result.contains_key("换发"));
        assert_eq!(result["出外"], "出外");
        assert_eq!(result["干燥"], "幹燥");
        assert_eq!(result["台北"], "臺北");
        assert_eq!(result["台面"], "枱面");
        assert_eq!(choices.len(), 1);
        assert_eq!(choices["干燥"].candidates, ["乾燥", "干燥"]);
        assert_eq!(choices["干燥"].output, "幹燥");
        assert!(rules.apply(&format!("{source}新增\t甲 乙 丙\n")).is_err());
        assert!(rules.apply(&format!("{source}出外\t出外\n")).is_err());
        assert!(rules.apply("出外\t出外\n").is_err());
    }
}
