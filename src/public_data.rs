//! 資料目錄、初始範本及宿主快取。
use std::path::Path;
use std::sync::Arc;

use crate::diagnostics::{self, Level};
use crate::project_files::{self, FileStamp};

pub(crate) const CHARACTERS: &str = "t2s/TSCharacters.txt";
pub(crate) const PHRASES: &str = "t2s/TSPhrases.txt";
pub(crate) const S2T_PHRASES: &str = "s2t/s2t-phrase.min.json";
pub(crate) const S2T: [&str; 3] = ["s2t/custom.json", S2T_PHRASES, "s2t/s2t-char.min.json"];
pub(crate) const JYUTPING: &str = "jyut6ping3.chars.dict.csv.json";
pub(crate) const RANKING: &str = "common-rank.txt";
pub(crate) const CANGJIE: &str = "cangjie3.dict.yaml";
pub(crate) const CUSTOM: &str = "custom.dict.yaml";
pub(crate) const STROKE: &str = "stroke.dict.yaml";
pub(crate) const FUZZY: &str = "homophone.fuzzy.yaml";
pub(crate) const CUHK_RANK: &str = "build/cuhk-rank.txt";
pub(crate) const S2T_RULES: [&str; 3] = [
    "build/s2t-rules/STPhrasesMultiOverrideHK.json",
    "build/s2t-rules/uncommon-fonts.json",
    "build/s2t-rules/exclusions.json",
];
pub(crate) const BUILD_TEMPLATES: [&str; 4] = [CUHK_RANK, S2T_RULES[0], S2T_RULES[1], S2T_RULES[2]];
pub(crate) const MAX_BYTES: u64 = 16 * 1024 * 1024;

pub(crate) const INITIAL_CHARACTERS: &str =
    include_str!("../assets/templates/t2s/TSCharacters.txt");
pub(crate) const INITIAL_PHRASES: &str = include_str!("../assets/templates/t2s/TSPhrases.txt");
pub(crate) const INITIAL_JYUTPING: &str =
    include_str!("../assets/templates/jyut6ping3.chars.dict.csv.json");
pub(crate) const INITIAL_RANKING: &str = include_str!("../assets/templates/common-rank.txt");
pub(crate) const CUSTOM_TEMPLATE: (&str, &[u8]) = (
    CUSTOM,
    include_bytes!("../assets/templates/custom.dict.yaml"),
);
static STROKE_BYTES: [u8; include_bytes!("../assets/templates/stroke.dict.yaml").len()] =
    *include_bytes!("../assets/templates/stroke.dict.yaml");
pub(crate) const STROKE_TEMPLATE: (&str, &[u8]) = (STROKE, &STROKE_BYTES);
pub(crate) const TEMPLATES: &[(&str, &[u8])] = &[
    (
        S2T[0],
        include_bytes!("../assets/templates/s2t/custom.json"),
    ),
    (
        S2T[1],
        include_bytes!("../assets/templates/s2t/s2t-phrase.min.json"),
    ),
    (
        S2T[2],
        include_bytes!("../assets/templates/s2t/s2t-char.min.json"),
    ),
    (CHARACTERS, INITIAL_CHARACTERS.as_bytes()),
    (PHRASES, INITIAL_PHRASES.as_bytes()),
    (
        CANGJIE,
        include_bytes!("../assets/templates/cangjie3.dict.yaml"),
    ),
    CUSTOM_TEMPLATE,
    STROKE_TEMPLATE,
    (JYUTPING, INITIAL_JYUTPING.as_bytes()),
    (
        FUZZY,
        include_bytes!("../assets/templates/homophone.fuzzy.yaml"),
    ),
    (RANKING, INITIAL_RANKING.as_bytes()),
];

pub(crate) fn current_directory() -> Option<&'static Path> {
    use std::path::PathBuf;
    use std::sync::OnceLock;
    static DIRECTORY_PATH: OnceLock<Option<PathBuf>> = OnceLock::new();
    DIRECTORY_PATH
        .get_or_init(|| {
            let root = project_files::runtime_root()?;
            Some(project_files::user_data(root).ok()?.join("dictionaries"))
        })
        .as_deref()
}

pub(crate) struct Cache<T, const N: usize = 1> {
    attempted: Option<[FileStamp; N]>,
    value: Option<Arc<T>>,
}

impl<T, const N: usize> Cache<T, N> {
    pub(crate) const fn empty() -> Self {
        Self {
            attempted: None,
            value: None,
        }
    }

    pub(crate) fn invalidate(&mut self) {
        self.attempted = None;
    }

    pub(crate) fn get(
        &mut self,
        directory: &Path,
        files: [&str; N],
        dataset: &'static str,
        parse: impl FnOnce([&str; N]) -> Result<T, String>,
    ) -> Option<Arc<T>> {
        let Some((before, sources)) = self.read_changed(directory, files) else {
            return self.value.clone();
        };
        let result = parse(sources.each_ref().map(String::as_str));
        self.attempted = Some(before);
        diagnostics::event(
            if result.is_ok() {
                Level::Info
            } else {
                Level::Warning
            },
            "public_data",
            dataset,
            &[
                ("valid", i64::from(result.is_ok())),
                (
                    "keep_previous",
                    i64::from(result.is_err() && self.value.is_some()),
                ),
            ],
        );
        if let Ok(value) = result {
            self.value = Some(Arc::new(value));
        }
        self.value.clone()
    }

    fn read_changed(
        &self,
        directory: &Path,
        files: [&str; N],
    ) -> Option<([FileStamp; N], [String; N])> {
        let paths = files.map(|name| directory.join(name));
        let stamps = || {
            read_array(paths.each_ref(), |path| {
                FileStamp::read(path)
                    .ok()
                    .filter(|stamp| stamp.size <= MAX_BYTES)
            })
        };
        let before = stamps()?;
        if self.attempted.as_ref() == Some(&before) {
            return None;
        }

        let guard = crate::update::publication_reader(directory).ok()?;
        let sources = read_array(paths.each_ref(), |path| std::fs::read_to_string(path).ok())?;
        // 首次發佈可能在讀取前後建立鎖檔，此時須取得同一把鎖再核對檔案。
        let _guard = match guard {
            Some(guard) => Some(guard),
            None => crate::update::publication_reader(directory).ok()?,
        };
        (stamps()? == before).then_some((before, sources))
    }
}

fn read_array<I, T, const N: usize>(
    inputs: [I; N],
    mut read: impl FnMut(I) -> Option<T>,
) -> Option<[T; N]> {
    let mut values = [const { None }; N];
    for (slot, input) in values.iter_mut().zip(inputs) {
        *slot = Some(read(input)?);
    }
    Some(values.map(|value| value.expect("已讀取所有檔案")))
}
