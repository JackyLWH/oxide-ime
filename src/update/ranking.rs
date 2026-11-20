//! 沿用原來香港常用字排名的加權倒數名次。
use super::{files, message, status, Failure, Update};
use crate::public_data;
use num_rational::Ratio;
use std::collections::{HashMap, HashSet};

const WORDS_HK: &str = "https://words.hk/faiman/analysis";
const GENRES: [&str; 4] = ["Written", "SpokenAdult", "SpokenChildren", "AdultToChild"];
const WEIGHTS: [u128; 6] = [60, 30, 4, 4, 1, 1];

type Ranks = HashMap<char, usize>;
type Counts = HashMap<char, u64>;

fn unified(ch: char) -> bool {
    // Unicode 16.0 的統一漢字範圍。
    matches!(ch as u32,
        0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0x20000..=0x2A6DF | 0x2A700..=0x2B739
        | 0x2B740..=0x2B81D | 0x2B820..=0x2CEA1 | 0x2CEB0..=0x2EBE0 | 0x2EBF0..=0x2EE5D
        | 0x30000..=0x3134A | 0x31350..=0x323AF)
}

fn one_han(value: &str) -> Option<char> {
    let mut chars = value.chars();
    let ch = chars.next()?;
    (chars.next().is_none()
        && (unified(ch)
            || ch == '〇'
            || matches!(ch as u32, 0xF900..=0xFA6D | 0xFA70..=0xFAD9 | 0x2F800..=0x2FA1D)))
    .then_some(ch)
}

fn frequency_ranks(counts: Counts) -> Ranks {
    let mut sorted: Vec<_> = counts.into_iter().collect();
    sorted.sort_unstable_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    let (mut previous, mut rank) = (None, 0);
    sorted
        .into_iter()
        .enumerate()
        .map(|(i, (ch, frequency))| {
            if previous != Some(frequency) {
                previous = Some(frequency);
                rank = i + 1;
            }
            (ch, rank)
        })
        .collect()
}

fn cifu_counts(source: &str) -> Result<[Counts; 4], String> {
    let mut reader = csv::ReaderBuilder::new()
        .delimiter(b'\t')
        .from_reader(source.as_bytes());
    let headers = reader.headers().map_err(message)?;
    let column = |name: &str| {
        headers
            .iter()
            .position(|header| header == name)
            .ok_or_else(|| format!("Cifu 缺少 {name}"))
    };
    let word = column("Word")?;
    let genres = [
        column(GENRES[0])?,
        column(GENRES[1])?,
        column(GENRES[2])?,
        column(GENRES[3])?,
    ];
    let mut counts: [Counts; 4] = std::array::from_fn(|_| HashMap::new());
    for row in reader.records() {
        let row = row.map_err(message)?;
        for (counts, index) in counts.iter_mut().zip(genres) {
            let frequency: u64 = row[index].parse().map_err(message)?;
            if frequency == 0 {
                continue;
            }
            for ch in row[word].chars().filter(|&ch| unified(ch)) {
                let value = counts.entry(ch).or_default();
                *value = value
                    .checked_add(frequency)
                    .ok_or("Cifu 字頻超出有效範圍")?;
            }
        }
    }
    if counts.iter().any(|table| table.len() < 1_000) {
        return Err("Cifu 字頻資料不完整".into());
    }
    Ok(counts)
}

fn supplements(source: &str) -> Result<HashMap<char, usize>, String> {
    let grades = ["A", "1", "2", "B", "3", "4", "C", "5", "6"];
    let mut active = None;
    let mut result = HashMap::new();
    for line in source.trim_start_matches('\u{feff}').lines() {
        if let Some((grade, _)) = line
            .strip_prefix('【')
            .and_then(|line| line.split_once("級】"))
        {
            active = grades.iter().position(|&value| value == grade);
        }
        if let Some((code, text)) = line.split_once('\t') {
            if let (Some(grade), Ok(code)) = (active, u32::from_str_radix(code, 16)) {
                let ch = char::from_u32(code).ok_or("香港外字表碼位無效")?;
                let text = text.split_whitespace().next().and_then(one_han);
                if text != Some(ch) {
                    return Err("香港外字表的字與碼位不符".into());
                }
                let best = result.entry(ch).or_insert(grade);
                *best = (*best).min(grade);
            }
        }
    }
    if result.len() < 100 {
        return Err("香港外字表不完整".into());
    }
    Ok(result)
}

fn build(counts: &str, words: &str, cifu: &str, supp: &str, cuhk: &str) -> Result<String, String> {
    let counts: HashMap<String, u64> = serde_json::from_str(counts).map_err(message)?;
    let counts: Counts = counts
        .into_iter()
        .filter_map(|(ch, count)| Some((one_han(&ch)?, count)))
        .collect();
    let words: HashMap<String, serde::de::IgnoredAny> =
        serde_json::from_str(words).map_err(message)?;
    let lexicon: HashSet<_> = words.keys().filter_map(|word| one_han(word)).collect();
    if counts.len() < 4_000 || lexicon.len() < 4_000 || counts.values().any(|&count| count == 0) {
        return Err("粵典字頻或單字表不完整".into());
    }
    let cuhk: Ranks = cuhk
        .lines()
        .filter_map(one_han)
        .enumerate()
        .map(|(i, ch)| (ch, i + 1))
        .collect();
    if cuhk.len() != 4_627 {
        return Err("中大排名的固定快照無效".into());
    }
    let [written, adult, children, to_child] = cifu_counts(cifu)?.map(frequency_ranks);
    let rank = [
        frequency_ranks(counts),
        cuhk,
        written,
        adult,
        children,
        to_child,
    ];
    let grades = supplements(supp)?;
    let mut chars: HashSet<_> = lexicon.clone();
    chars.extend(grades.keys());
    for table in &rank {
        chars.extend(table.keys());
    }
    if chars.len() > 200_000 {
        return Err("常用字排名超出資料上限".into());
    }
    let get = |group: usize, ch: char| rank[group].get(&ch).copied().unwrap_or(usize::MAX);
    let mut scored: Vec<_> = chars
        .into_iter()
        .map(|ch| {
            // 六個分母均不超過 200060；u128 可保存精確的和。
            let score: Ratio<u128> = rank
                .iter()
                .zip(WEIGHTS)
                .filter_map(|(group, weight)| {
                    Some(Ratio::new(weight, 60 + *group.get(&ch)? as u128))
                })
                .sum();
            (ch, score)
        })
        .collect();
    scored.sort_unstable_by(|(a, score_a), (b, score_b)| {
        score_b
            .cmp(score_a)
            .then_with(|| {
                if *score_a.numer() != 0 {
                    (get(0, *a), get(1, *a)).cmp(&(get(0, *b), get(1, *b)))
                } else {
                    let fallback = |ch: char| {
                        (
                            grades.get(&ch).copied().unwrap_or(usize::MAX),
                            !lexicon.contains(&ch),
                        )
                    };
                    fallback(*a).cmp(&fallback(*b))
                }
            })
            .then_with(|| a.cmp(b))
    });
    let mut result = String::new();
    for (ch, _) in scored {
        result.push(ch);
        result.push('\n');
    }
    validate_ranking(&result)?;
    Ok(result)
}

fn validate_ranking(source: &str) -> Result<crate::dictionary::Ranking, String> {
    let ranks = crate::dictionary::Ranking::parse(source)?;
    if ranks.len() < 4_000 {
        return Err("常用字排名不完整".into());
    }
    Ok(ranks)
}

pub(super) fn update(update: &Update<'_>) -> Result<String, Failure> {
    let [cuhk] = files::read_build_inputs(update.data, [public_data::CUHK_RANK])?;
    let download = &update.source;
    let count = download.get(&format!("{WORDS_HK}/charcount.json"))?;
    let list = download.get(&format!("{WORDS_HK}/charlist.json"))?;
    let cifu = download.latest("gwinterstein/Cifu", "master")?;
    let supplement = download.latest("ichitenfont/suppchara", "master")?;
    let contents = build(
        &count,
        &list,
        &cifu.file("Lexicon/Cifu-v1.txt")?,
        &supplement.file("suppchara.txt")?,
        &cuhk,
    )?;
    let changed = files::replace(
        update.data,
        update.setting,
        &[(public_data::RANKING, contents.as_bytes())],
        &[(public_data::CUHK_RANK, &cuhk)],
    )?;
    Ok(status(
        changed,
        &format!(
            "粵典，Cifu {}，香港外字表 {}，中大快照",
            cifu.sha, supplement.sha
        ),
    ))
}

#[cfg(test)]
mod tests {
    use std::fmt::Write;

    #[test]
    fn combines_frequency_ranks_and_keeps_unranked_supplements_in_grade_order() {
        let mut counts = super::Counts::new();
        let mut words = std::collections::HashMap::new();
        let mut cifu = String::from("Word\tWritten\tSpokenAdult\tSpokenChildren\tAdultToChild\n");
        for index in 0..4_000 {
            let ch = char::from_u32(0x20100 + index).unwrap();
            counts.insert(ch, if index == 0 { 2 } else { 1 });
            words.insert(ch, ());
            if index < 1_000 {
                writeln!(cifu, "{ch}\t1\t1\t1\t1").unwrap();
            }
        }
        let mut supplements = String::from("【B級】\n");
        for code in 0x22000..0x22064 {
            writeln!(supplements, "{code:X}\t{}", char::from_u32(code).unwrap()).unwrap();
        }
        supplements.push_str("【A級】\n22100\t\u{22100}\n");
        let cuhk = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("assets/templates")
                .join(crate::public_data::CUHK_RANK),
        )
        .unwrap();
        let ranked = super::build(
            &serde_json::to_string(&counts).unwrap(),
            &serde_json::to_string(&words).unwrap(),
            &cifu,
            &supplements,
            &cuhk,
        )
        .unwrap();
        let ranks = super::validate_ranking(&ranked).unwrap();
        assert!(ranks.rank("\u{20100}") < ranks.rank("\u{20101}"));
        assert!(ranks.rank("\u{20101}") < ranks.rank("\u{20102}"));
        assert!(ranks.rank("\u{22100}") < ranks.rank("\u{22000}"));
    }
}
