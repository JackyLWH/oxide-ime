//! 先按 CanCLID 分類建立單字庫，再由可逐字對齊的 Rime 詞音補音。
use std::collections::HashMap;

use fancy_regex::Regex;

use crate::homophone::{valid_reading, Data, Ordered, Status};

type Readings = Vec<(String, Status)>;
type Characters = Vec<(String, Readings)>;

fn character_rows(source: &str) -> Result<Characters, String> {
    let mut chars: Characters = Vec::new();
    let mut positions = HashMap::new();
    let mut reader = csv::ReaderBuilder::new()
        .has_headers(false)
        .flexible(true)
        .from_reader(source.trim_start_matches('\u{feff}').as_bytes());
    for (line, row) in reader.records().enumerate() {
        let row = row.map_err(super::message)?;
        let text = row.get(0).unwrap_or_default().trim();
        let reading = row.get(1).unwrap_or_default().trim();
        if row.iter().all(|field| field.trim().is_empty())
            || text.starts_with('#')
            || (text == "char" && reading == "jyutping")
        {
            continue;
        }
        let status = row
            .get(2)
            .and_then(|label| Status::from_label(label.trim()));
        if text.chars().count() != 1 || !valid_reading(reading) || status.is_none() {
            return Err(format!("CanCLID 第 {} 列嘅字、讀音或分類無效", line + 1));
        }
        let status = status.unwrap();
        let index = *positions.entry(text.to_owned()).or_insert_with(|| {
            let index = chars.len();
            chars.push((text.to_owned(), Vec::new()));
            index
        });
        let readings = &mut chars[index].1;
        if let Some((_, old_status)) = readings.iter().find(|(old, _)| old == reading) {
            if *old_status != status {
                return Err(format!("CanCLID 讀音分類衝突：{text} {reading}"));
            }
        } else {
            readings.push((reading.to_owned(), status));
        }
    }
    for (_, readings) in &mut chars {
        readings.sort_by_key(|(_, status)| *status);
    }
    Ok(chars)
}

// 上游詞典以讀音排列。補音工具先按詞彙第一次出現的次序分組。
fn words(source: &str) -> Result<Vec<(&str, Vec<&str>)>, String> {
    let (_, body) = source.split_once("\n...").ok_or("Rime 粵拼詞典缺少表頭")?;
    let mut words: Vec<(&str, Vec<&str>)> = Vec::new();
    let mut positions = HashMap::new();
    for line in body
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
    {
        let mut fields = line.split('\t').map(str::trim);
        let (text, reading) = fields
            .next()
            .zip(fields.next())
            .ok_or("Rime 粵拼詞典缺少欄位")?;
        if text.is_empty() || reading.is_empty() {
            return Err("Rime 粵拼詞典有空白欄位".into());
        }
        let index = *positions.entry(text).or_insert_with(|| {
            let index = words.len();
            words.push((text, Vec::new()));
            index
        });
        words[index].1.push(reading);
    }
    Ok(words)
}

fn enrich(chars: &mut Characters, words: Vec<(&str, Vec<&str>)>) -> Result<(), String> {
    let mut positions: HashMap<_, _> = chars
        .iter()
        .enumerate()
        .filter_map(|(index, (key, _))| key.chars().next().map(|ch| (ch, index)))
        .collect();
    let han = Regex::new(r"[\p{Unified_Ideograph}\u{f900}-\u{faff}\u{2f800}-\u{2fa1f}〇]")
        .map_err(super::message)?;
    let punctuation = Regex::new(r"[\p{P}\s]").map_err(super::message)?;
    for (word, readings) in words {
        let text = punctuation.replace_all(word, "");
        let characters: Vec<_> = text.chars().collect();
        if characters.is_empty()
            || text.chars().any(|ch| ch.is_ascii())
            || han.find_iter(&text).count() != characters.len()
        {
            continue;
        }
        for reading in readings {
            let reading = reading.to_ascii_lowercase();
            let syllables: Vec<_> = reading.split_whitespace().collect();
            if syllables.len() != characters.len()
                || !syllables.iter().all(|part| valid_reading(part))
            {
                continue;
            }
            for (&ch, code) in characters.iter().zip(syllables) {
                let index = *positions.entry(ch).or_insert_with(|| {
                    let index = chars.len();
                    chars.push((ch.to_string(), Vec::new()));
                    index
                });
                let readings = &mut chars[index].1;
                if !readings.iter().any(|(old, _)| old == code) {
                    readings.push((code.to_owned(), Status::Common));
                }
            }
        }
    }
    for (digit, character) in ('0'..='9').zip("零一二三四五六七八九".chars()) {
        if !positions.contains_key(&digit) {
            if let Some(&index) = positions.get(&character) {
                let aliases = chars[index]
                    .1
                    .iter()
                    .map(|(code, _)| (code.clone(), Status::Common))
                    .collect();
                chars.push((digit.to_string(), aliases));
            }
        }
    }
    Ok(())
}

pub(super) fn build(csv: &str, yaml: &str) -> Result<String, String> {
    let mut chars = character_rows(csv)?;
    let words = words(yaml)?;
    if chars.len() < 20_000 || words.len() < 50_000 {
        return Err("上游粵拼字庫或詞庫不完整".into());
    }
    enrich(&mut chars, words)?;
    let entries = Ordered(
        chars
            .into_iter()
            .map(|(text, readings)| (text, Ordered(readings)))
            .collect(),
    );
    let result = serde_json::to_string_pretty(&entries).map_err(super::message)? + "\n";
    Data::from_public_json(&result)?;
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enriches_in_word_order_and_skips_mixed_or_unaligned_words() {
        let mut chars =
            character_rows("char,jyutping,pron_rank\n一,jat6,罕見\n一,jat1,預設\n甲,gaap3,預設\n")
                .unwrap();
        let words = words("---\n...\n甲・乙\tgaap1 jyut3\n甲A\tgaap2 a1\n乙甲\tjyut2 gaap4\n甲・乙\tgaap5 jyut5\n甲乙\tgaap6\n").unwrap();
        enrich(&mut chars, words).unwrap();
        let readings = |index: usize| {
            chars[index]
                .1
                .iter()
                .map(|(reading, _)| reading.as_str())
                .collect::<Vec<_>>()
        };
        assert_eq!(readings(0), ["jat1", "jat6"]);
        assert_eq!(readings(1), ["gaap3", "gaap1", "gaap5", "gaap4"]);
        assert_eq!(readings(2), ["jyut3", "jyut5", "jyut2"]);
        assert_eq!(chars[3].0, "1");
        assert_eq!(readings(3), ["jat1", "jat6"]);
        assert!(chars[3]
            .1
            .iter()
            .all(|(_, status)| *status == Status::Common));
    }
}
