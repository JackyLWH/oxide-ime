//! 首次登入時建立資料。更新與重裝都沿用已初始化的目錄。
use std::fs;
use std::io;
use std::path::Path;

use super::windows;
use crate::public_data;

pub(super) const GROUPS: [&str; 5] = ["opencc", "s2t", "jyutping", "ranking", "cangjie"];
pub(super) const DEFAULT_POLICY: &str = "opencc,s2t,jyutping,ranking,cangjie";
pub(super) const POLICY: &str = "update-policy.txt";
pub(super) const INTERVAL: &str = "update-interval.txt";
pub(super) const REPORT: &str = "last-update.txt";

pub(super) fn initialise(
    data: &Path,
    docs: &Path,
    templates: Option<&Path>,
    selection: &str,
) -> Result<Vec<String>, String> {
    let policy = if selection == "-" {
        String::new()
    } else {
        let groups: Vec<_> = selection.split(',').collect();
        if !groups.iter().all(|group| GROUPS.contains(group)) {
            return Err("無效的公用資料選項".into());
        }
        groups.join("\n") + "\n"
    };
    initialise_at(data, docs, templates, &policy)
}

fn initialise_at(
    data: &Path,
    docs: &Path,
    templates: Option<&Path>,
    policy: &str,
) -> Result<Vec<String>, String> {
    let Some(_lock) = windows::Lock::acquire_for("InitializeData", 5_000)? else {
        return Err("另一個維護程序正在初始化使用者資料".into());
    };
    if data.join(".initialized").is_file() {
        return crate::update::register_initial_sources(data, templates, false, write_new);
    }
    let mut notices = Vec::new();
    let dictionaries = data.join("dictionaries");
    for &(name, bytes) in public_data::TEMPLATES {
        let path = dictionaries.join(name);
        write_new(&path, bytes)?;
        if !same_contents(&path, bytes) {
            notices.push(name.to_owned());
        }
    }
    for (name, bytes) in [
        ("startup-mode.txt", &b"en\n"[..]),
        (POLICY, policy.as_bytes()),
        (INTERVAL, &b"7\n"[..]),
    ] {
        write_new(&data.join("setting").join(name), bytes)?;
    }
    write_new(
        &docs.join("安裝及使用.md"),
        include_bytes!("../../docs/installation.md"),
    )?;
    notices.extend(crate::update::register_initial_sources(
        data, templates, true, write_new,
    )?);
    Ok(notices)
}

fn same_contents(path: &Path, expected: &[u8]) -> bool {
    fs::metadata(path).is_ok_and(|metadata| metadata.len() == expected.len() as u64)
        && fs::read(path).is_ok_and(|bytes| bytes == expected)
}

fn error(error: impl std::fmt::Display) -> String {
    error.to_string()
}

fn write_new(path: &Path, contents: &[u8]) -> Result<(), String> {
    if path.is_file() {
        return Ok(());
    }
    let parent = path.parent().ok_or("無法取得上層資料目錄")?;
    fs::create_dir_all(parent).map_err(error)?;
    match crate::project_files::write_staged(path, contents, windows::publish_initial) {
        Err(err) if err.kind() == io::ErrorKind::AlreadyExists && path.is_file() => Ok(()),
        result => result.map_err(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn templates() -> &'static Path {
        Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/assets/templates"))
    }

    #[test]
    fn fresh_initialisation_creates_only_the_dictionaries_and_settings() {
        let root = crate::tests::fixture_dir("fresh-data-structure");
        let data = root.join("data");
        let docs = root.join("docs");
        assert!(initialise(&data, &docs, Some(templates()), DEFAULT_POLICY)
            .unwrap()
            .is_empty());
        let names = |dir: &Path| {
            let mut files: Vec<_> = fs::read_dir(dir)
                .unwrap()
                .map(|item| item.unwrap().file_name().into_string().unwrap())
                .collect();
            files.sort();
            files
        };
        assert_eq!(names(&data), [".initialized", "dictionaries", "setting"]);
        assert_eq!(
            names(&data.join("setting")),
            [
                "startup-mode.txt",
                "update-interval.txt",
                "update-policy.txt"
            ]
        );
        assert_eq!(
            fs::read(data.join("setting").join(POLICY)).unwrap(),
            b"opencc\ns2t\njyutping\nranking\ncangjie\n"
        );
        assert_eq!(
            fs::read(data.join("setting").join(INTERVAL)).unwrap(),
            b"7\n"
        );
        assert_eq!(
            fs::read(data.join("setting/startup-mode.txt")).unwrap(),
            b"en\n"
        );
        assert_eq!(
            fs::read(docs.join("安裝及使用.md")).unwrap(),
            include_bytes!("../../docs/installation.md")
        );
        crate::tests::remove_fixture(&root);
    }

    #[test]
    fn initialisation_seeds_all_templates_and_preserves_edits_and_deletions() {
        let root = crate::tests::fixture_dir("installation-data");
        let data = root.join("data");
        let docs = root.join("docs");
        let dictionaries = data.join("dictionaries");
        fs::create_dir_all(&dictionaries).unwrap();
        let private = dictionaries.join(public_data::CUSTOM);
        fs::write(&private, "私人詞條").unwrap();
        assert_eq!(
            initialise_at(&data, &docs, Some(templates()), "opencc\n").unwrap(),
            [public_data::CUSTOM]
        );
        assert_eq!(fs::read_to_string(private).unwrap(), "私人詞條");
        for &(name, expected) in public_data::TEMPLATES {
            if name != public_data::CUSTOM {
                assert_eq!(
                    fs::read(dictionaries.join(name)).unwrap(),
                    expected,
                    "{name}"
                );
            }
        }
        assert!(docs.join("安裝及使用.md").is_file());
        assert_eq!(
            fs::read(data.join("setting").join(POLICY)).unwrap(),
            b"opencc\n"
        );
        fs::remove_file(dictionaries.join(public_data::FUZZY)).unwrap();
        let removed = dictionaries.join(public_data::CUHK_RANK);
        fs::remove_file(&removed).unwrap();
        initialise_at(&data, &docs, Some(templates()), "ranking\n").unwrap();
        assert!(!removed.exists());
        assert!(!dictionaries.join(public_data::FUZZY).exists());
        assert_eq!(
            fs::read(data.join("setting").join(POLICY)).unwrap(),
            b"opencc\n"
        );
        crate::tests::remove_fixture(&root);
    }

    #[test]
    fn existing_baseline_gains_build_sources_without_replacing_user_files() {
        let root = crate::tests::fixture_dir("build-source-migration");
        let data = root.join("data");
        let docs = root.join("docs");
        let dictionaries = data.join("dictionaries");
        let edited = dictionaries.join(public_data::CUHK_RANK);
        fs::create_dir_all(edited.parent().unwrap()).unwrap();
        fs::write(&edited, "原有自訂排名").unwrap();
        let legacy = dictionaries.join("cangjie3_custom.dict.yaml");
        fs::write(&legacy, "...\n原有附加詞條\tabc\n").unwrap();
        let marker = data.join(".initialized");
        fs::write(
            &marker,
            r#"{"format":1,"sha256":{"common-rank.txt":"last-update"}}"#,
        )
        .unwrap();

        assert_eq!(
            initialise_at(&data, &docs, Some(templates()), "opencc\n").unwrap(),
            [public_data::CUHK_RANK],
        );
        assert_eq!(fs::read_to_string(edited).unwrap(), "原有自訂排名");
        let custom = dictionaries.join(public_data::CUSTOM);
        assert_eq!(fs::read(&custom).unwrap(), public_data::CUSTOM_TEMPLATE.1);
        assert_eq!(
            fs::read_to_string(legacy).unwrap(),
            "...\n原有附加詞條\tabc\n"
        );
        fs::write(&custom, "...\n新自訂詞條\tabc\n").unwrap();
        assert!(initialise_at(&data, &docs, Some(templates()), "opencc\n")
            .unwrap()
            .is_empty());
        assert_eq!(
            fs::read_to_string(custom).unwrap(),
            "...\n新自訂詞條\tabc\n"
        );
        let stored: serde_json::Value = serde_json::from_slice(&fs::read(marker).unwrap()).unwrap();
        assert_eq!(stored["sha256"]["common-rank.txt"], "last-update");
        assert_eq!(
            stored["sha256"][public_data::CUSTOM]
                .as_str()
                .unwrap()
                .len(),
            64
        );
        for name in public_data::BUILD_TEMPLATES {
            assert_eq!(stored["sha256"][name].as_str().unwrap().len(), 64);
            if name != public_data::CUHK_RANK {
                assert_eq!(
                    fs::read(dictionaries.join(name)).unwrap(),
                    fs::read(templates().join(name)).unwrap(),
                );
            }
        }
        crate::tests::remove_fixture(&root);
    }
}
