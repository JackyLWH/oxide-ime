//! 各宿主程序按專案字典的檔案狀態重新載入，失敗時保留上一份有效字庫。
use std::path::{Path, PathBuf};
use std::sync::{Arc, Weak};
use std::time::Instant;

use super::{Dictionary, InputMode, Ranking, DICTIONARY_FILES};
use crate::diagnostics::{self, Level, WarningLimit};
use crate::homophone::Fuzzy;
use crate::project_files::{self, FileStamp};
use crate::public_data::{self, Cache};

#[derive(Clone)]
pub(crate) struct Dictionaries {
    pub cangjie: Arc<Dictionary>,
    pub stroke: Option<Arc<Dictionary>>,
    pub ranking: Arc<Ranking>,
}

impl Dictionaries {
    pub(crate) fn table(&self, mode: InputMode) -> Option<&Arc<Dictionary>> {
        if mode == InputMode::Stroke {
            self.stroke.as_ref()
        } else {
            Some(&self.cangjie)
        }
    }
}

#[derive(PartialEq, Eq)]
struct Tables {
    base: FileStamp,
    custom: Option<FileStamp>,
}

pub(crate) struct DictionaryStore {
    directory: PathBuf,
    installed: bool,
    attempted: Option<Tables>,
    current: Option<Arc<Dictionary>>,
    stroke: Cache<Dictionary>,
    stroke_source: Weak<Dictionary>,
    ranking: Cache<Ranking>,
    fuzzy_stamp: Option<FileStamp>,
    fuzzy: Option<Arc<Fuzzy>>,
    fuzzy_config: Option<String>,
}

impl DictionaryStore {
    #[cfg(test)]
    pub(crate) fn new(directory: PathBuf) -> Self {
        Self::at(directory, false)
    }

    pub(crate) fn for_root(root: &Path) -> Result<Self, String> {
        Ok(Self::at(
            project_files::user_data(root)?.join("dictionaries"),
            project_files::installed(root),
        ))
    }

    fn at(directory: PathBuf, installed: bool) -> Self {
        Self {
            directory,
            installed,
            attempted: None,
            current: None,
            stroke: Cache::empty(),
            stroke_source: Weak::new(),
            ranking: Cache::empty(),
            fuzzy_stamp: None,
            fuzzy: None,
            fuzzy_config: None,
        }
    }

    pub(crate) fn dictionaries(&mut self, mode: InputMode) -> Result<Dictionaries, String> {
        let cangjie = self.refresh()?;
        let stroke = if mode == InputMode::Stroke {
            if !self.stroke_source.upgrade().is_some_and(|previous| {
                Arc::ptr_eq(&previous, &cangjie)
                    || previous.custom_entries() == cangjie.custom_entries()
            }) {
                self.stroke.invalidate();
            }
            self.stroke_source = Arc::downgrade(&cangjie);
            self.stroke.get(
                &self.directory,
                [public_data::STROKE],
                "stroke",
                |[source]| Dictionary::parse_stroke(source, &cangjie),
            )
        } else {
            None
        };
        let ranking = self
            .ranking
            .get(
                &self.directory,
                [public_data::RANKING],
                "ranking",
                |[source]| Ranking::parse(source),
            )
            .unwrap_or_else(Ranking::standard);
        Ok(Dictionaries {
            cangjie,
            stroke,
            ranking,
        })
    }

    fn stamps(base: &Path, custom: &Path) -> Result<Tables, String> {
        let base = FileStamp::read(base).map_err(|error| format!("無法讀取倉頡字庫（{error}）"))?;
        let custom = match FileStamp::read(custom) {
            Ok(stamp) => Some(stamp),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => return Err(format!("無法讀取私人詞庫：{error}")),
        };
        Ok(Tables { base, custom })
    }

    pub(crate) fn fuzzy(&mut self) -> Arc<Fuzzy> {
        let path = self.directory.join(crate::public_data::FUZZY);
        let data = crate::homophone::current();
        let stamp = FileStamp::read(&path)
            .ok()
            .filter(|stamp| stamp.size <= 64 * 1024);
        if self.fuzzy_stamp != stamp {
            let source = stamp
                .as_ref()
                .and_then(|_| std::fs::read_to_string(&path).ok());
            if let Some(source) = source {
                if FileStamp::read(&path).ok() == stamp {
                    self.fuzzy_stamp = stamp;
                    match Fuzzy::for_data(&source, data.clone()) {
                        Ok(fuzzy) => {
                            diagnostics::event(Level::Info, "dictionary", "fuzzy_loaded", &[]);
                            self.fuzzy_config = Some(source);
                            self.fuzzy = Some(fuzzy);
                        }
                        Err(_) => {
                            diagnostics::event(Level::Warning, "dictionary", "fuzzy_invalid", &[])
                        }
                    }
                }
            }
        }
        if let Some(value) = self.fuzzy.as_ref().filter(|value| value.uses(&data)) {
            return value.clone();
        }
        let fuzzy = self
            .fuzzy_config
            .as_ref()
            .and_then(|source| Fuzzy::for_data(source, data.clone()).ok())
            .unwrap_or_else(|| Fuzzy::standard_for(data));
        self.fuzzy = Some(fuzzy.clone());
        fuzzy
    }

    pub(crate) fn refresh(&mut self) -> Result<Arc<Dictionary>, String> {
        static FAILED_STAMP: WarningLimit = WarningLimit::new();
        let base = self.directory.join(DICTIONARY_FILES[0]);
        let custom = self.directory.join(DICTIONARY_FILES[1]);
        let before = match Self::stamps(&base, &custom) {
            Ok(stamps) => stamps,
            Err(error) => {
                if FAILED_STAMP.ready() {
                    diagnostics::event(
                        Level::Warning,
                        "dictionary",
                        "metadata_error",
                        &[("keep_previous", i64::from(self.current.is_some()))],
                    );
                }
                return self.current.clone().ok_or(error);
            }
        };
        if self.attempted.as_ref() == Some(&before) {
            if let Some(current) = &self.current {
                return Ok(current.clone());
            }
        }
        let custom_path = before.custom.as_ref().map(|_| custom.as_path());
        let started = Instant::now();
        let mut loaded = Dictionary::load(&base, custom_path);
        if loaded.is_err() && custom_path.is_some() && self.current.is_none() && self.installed {
            diagnostics::event(Level::Warning, "dictionary", "use_base_table", &[]);
            loaded = Dictionary::load(&base, None);
        }
        // 編輯器可能正在覆寫或替換檔案；不要發佈讀取期間改變的半成品。
        if Self::stamps(&base, &custom).ok().as_ref() != Some(&before) {
            diagnostics::event(Level::Detail, "dictionary", "changed_during_read", &[]);
            return self
                .current
                .clone()
                .ok_or_else(|| "字庫正在更新，請再試一次".into());
        }
        let has_custom = before.custom.is_some();
        self.attempted = Some(before);
        match loaded {
            Ok(dictionary) => {
                diagnostics::event(
                    Level::Info,
                    "dictionary",
                    "loaded",
                    &[
                        ("custom_available", i64::from(has_custom)),
                        ("entries", dictionary.entries.len() as i64),
                        ("elapsed_ms", diagnostics::millis(started.elapsed())),
                    ],
                );
                let dictionary = Arc::new(dictionary);
                self.current = Some(dictionary.clone());
                Ok(dictionary)
            }
            Err(error) => {
                if FAILED_STAMP.ready() {
                    diagnostics::event(
                        Level::Warning,
                        "dictionary",
                        "load_failed",
                        &[("keep_previous", i64::from(self.current.is_some()))],
                    );
                }
                self.current.clone().ok_or(error)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::{Key, Session};

    #[test]
    fn stroke_refreshes_shortcuts_and_keeps_the_last_valid_copy() {
        let dir = crate::tests::fixture_dir("stroke-shortcut-reload");
        std::fs::write(dir.join(DICTIONARY_FILES[0]), "...\n日\ta\n").unwrap();
        std::fs::write(dir.join(public_data::STROKE), "...\n一\th\n").unwrap();
        let custom = dir.join(DICTIONARY_FILES[1]);
        std::fs::write(&custom, "...\n舊詞條\tmynote\n").unwrap();
        let mut store = DictionaryStore::new(dir.clone());
        let text = |view: &Dictionaries| {
            let stroke = view.stroke.as_ref().unwrap();
            let item = stroke.lookup(InputMode::Stroke, "mynote", true, None, &view.ranking)[0];
            stroke.text(item, None).into_owned()
        };
        let original = store.dictionaries(InputMode::Stroke).unwrap();
        assert_eq!(text(&original), "舊詞條");
        std::fs::write(&custom, "...\n更新後詞條\tmynote\n").unwrap();
        let updated = store.dictionaries(InputMode::Stroke).unwrap();
        assert_eq!(text(&updated), "更新後詞條");
        assert_eq!(text(&original), "舊詞條");
        std::fs::write(&custom, "無效字庫").unwrap();
        let fallback = store.dictionaries(InputMode::Stroke).unwrap();
        assert!(Arc::ptr_eq(
            updated.stroke.as_ref().unwrap(),
            fallback.stroke.as_ref().unwrap()
        ));
        crate::tests::remove_fixture(&dir);
    }

    #[test]
    fn saved_changes_reload_and_invalid_intermediate_files_keep_the_last_good_table() {
        let dir = crate::tests::fixture_dir("dictionary-reload");
        let base = dir.join(DICTIONARY_FILES[0]);
        let custom = dir.join(DICTIONARY_FILES[1]);
        std::fs::write(&base, "---\n...\n日\ta\n").unwrap();
        std::fs::write(&custom, "---\n...\n").unwrap();
        let mut store = DictionaryStore::new(dir.clone());
        let first = store.refresh().unwrap();
        assert!(Arc::ptr_eq(&first, &store.refresh().unwrap()));
        std::fs::write(&custom, "---\n...\n第二版\ta\n").unwrap();
        let second = store.refresh().unwrap();
        assert!(!Arc::ptr_eq(&first, &second));
        let mut session = Session::new(second.clone());
        session.handle(Key::ToggleMode);
        session.handle(Key::Letter('a'));
        assert_eq!(session.candidates()[1].text, "第二版");
        std::fs::write(&custom, "---\n...\n格式無效\n").unwrap();
        assert!(Arc::ptr_eq(&second, &store.refresh().unwrap()));
        std::fs::write(&custom, "---\n...\n已修復的詞條\ta\n").unwrap();
        let fixed = store.refresh().unwrap();
        assert!(!Arc::ptr_eq(&second, &fixed));
        crate::tests::remove_fixture(&dir);
    }

    #[test]
    fn custom_dictionary_can_be_added_after_initial_load() {
        let dir = crate::tests::fixture_dir("dictionary-custom-optional");
        std::fs::write(dir.join(DICTIONARY_FILES[0]), "---\n...\n日\ta\n").unwrap();
        let mut store = DictionaryStore::new(dir.clone());
        let first = store.refresh().unwrap();
        let mut session = Session::new(first.clone());
        session.handle(Key::ToggleMode);
        session.handle(Key::Letter('a'));
        assert_eq!(session.candidates()[0].text, "日");
        std::fs::write(dir.join(DICTIONARY_FILES[1]), "這不是有效字典").unwrap();
        assert!(Arc::ptr_eq(&first, &store.refresh().unwrap()));
        std::fs::write(dir.join(DICTIONARY_FILES[1]), "---\n...\n詞\ta\n").unwrap();
        assert!(!Arc::ptr_eq(&first, &store.refresh().unwrap()));
        crate::tests::remove_fixture(&dir);
    }

    #[test]
    fn installed_commands_follow_the_user_dictionary() {
        use crate::commands::Command;

        let dir = crate::tests::fixture_dir("installed-commands");
        let base = dir.join(DICTIONARY_FILES[0]);
        let custom = dir.join(DICTIONARY_FILES[1]);
        std::fs::write(&base, "---\n...\n日\ta\n").unwrap();
        let mut store = DictionaryStore::at(dir.clone(), true);
        assert!(store
            .refresh()
            .unwrap()
            .lookup(InputMode::Cangjie, "z", true, None, &Ranking::default())
            .is_empty());

        std::fs::write(
            &custom,
            include_str!("../../assets/templates/custom.dict.yaml"),
        )
        .unwrap();
        let dictionary = store.refresh().unwrap();
        let commands = |prefix| {
            dictionary
                .lookup(InputMode::Cangjie, prefix, true, None, &Ranking::default())
                .into_iter()
                .map(|entry| dictionary.command(entry).unwrap())
                .collect::<Vec<_>>()
        };
        assert_eq!(
            commands("zv"),
            [Command::ClipboardRaw, Command::ClipboardConverted]
        );
        assert_eq!(commands("ztime"), [Command::Time]);
        assert_eq!(commands("zdate"), [Command::Date]);
        crate::tests::remove_fixture(&dir);
    }

    #[test]
    fn installed_store_reloads_a_replaced_table_and_keeps_its_last_good_copy() {
        let dir = crate::tests::fixture_dir("installed-dictionary");
        let table = dir.join(DICTIONARY_FILES[0]);
        std::fs::write(&table, "---\n...\n初\ta\n").unwrap();
        let mut store = DictionaryStore::at(dir.clone(), true);
        let get = |store: &mut DictionaryStore| {
            let dictionary = store.refresh().unwrap();
            let candidate =
                dictionary.lookup(InputMode::Cangjie, "a", true, None, &Ranking::default())[0];
            dictionary.text(candidate, None).into_owned()
        };
        assert_eq!(get(&mut store), "初");
        std::fs::write(&table, "---\n...\n更新\ta\n").unwrap();
        assert_eq!(get(&mut store), "更新");
        std::fs::write(&table, "---\n...\n使用者版本\ta\n").unwrap();
        assert_eq!(get(&mut store), "使用者版本");
        std::fs::write(&table, "格式無效").unwrap();
        assert_eq!(get(&mut store), "使用者版本");
        crate::tests::remove_fixture(&dir);
    }
}
