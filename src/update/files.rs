//! 防止更新覆蓋使用者的詞庫。每組驗證、發佈、提交基準或還原。
use std::borrow::Cow;
use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read};
use std::os::windows::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::ptr;

use serde::{Deserialize, Serialize};

use crate::public_data::{BUILD_TEMPLATES, CUSTOM_TEMPLATE, MAX_BYTES, TEMPLATES};
use crate::tsf::ffi::Raw;
use crate::win32::wide;

use super::message;

const MARKER: &str = ".initialized";
const PENDING: &str = ".dictionary-update.json";
const TRANSACTION_LOCK: &str = ".dictionary-update-lock";

#[link(name = "bcrypt")]
extern "system" {
    fn BCryptOpenAlgorithmProvider(
        handle: *mut Raw,
        id: *const u16,
        provider: *const u16,
        flags: u32,
    ) -> i32;
    fn BCryptHash(
        handle: Raw,
        secret: *const u8,
        secret_len: u32,
        input: *const u8,
        len: u32,
        output: *mut u8,
        output_len: u32,
    ) -> i32;
    fn BCryptCloseAlgorithmProvider(handle: Raw, flags: u32) -> i32;
}

fn digest(bytes: &[u8]) -> Result<String, String> {
    use std::fmt::Write as _;
    let len = u32::try_from(bytes.len()).map_err(message)?;
    let mut handle = ptr::null_mut();
    let status = unsafe {
        BCryptOpenAlgorithmProvider(&mut handle, wide("SHA256").as_ptr(), ptr::null(), 0)
    };
    if status < 0 {
        return Err(format!("無法開啟 SHA-256（0x{:08X}）", status as u32));
    }
    let mut hash = [0u8; 32];
    let status = unsafe {
        BCryptHash(
            handle,
            ptr::null(),
            0,
            bytes.as_ptr(),
            len,
            hash.as_mut_ptr(),
            hash.len() as u32,
        )
    };
    unsafe { BCryptCloseAlgorithmProvider(handle, 0) };
    if status < 0 {
        return Err(format!("無法計算 SHA-256（0x{:08X}）", status as u32));
    }
    let mut hex = String::with_capacity(64);
    for byte in hash {
        write!(hex, "{byte:02x}").expect("寫入字串應成功");
    }
    Ok(hex)
}

#[derive(Deserialize, Serialize)]
struct Baseline {
    format: u32,
    sha256: BTreeMap<String, String>,
}

impl Baseline {
    fn read(data: &Path) -> Result<Self, String> {
        let source = fs::read(data.join(MARKER)).map_err(message)?;
        let value: Self = serde_json::from_slice(&source).map_err(message)?;
        if value.format != 1 {
            return Err("無法辨識資料目錄的字庫更新基準".into());
        }
        Ok(value)
    }

    fn write(&self, data: &Path) -> Result<(), String> {
        let mut bytes = serde_json::to_vec_pretty(self).map_err(message)?;
        bytes.push(b'\n');
        write_atomic(&data.join(MARKER), &bytes)
    }
}

fn initial_baseline() -> Result<Baseline, String> {
    Ok(Baseline {
        format: 1,
        sha256: TEMPLATES
            .iter()
            .map(|(name, bytes)| Ok(((*name).to_owned(), digest(bytes)?)))
            .collect::<Result<_, String>>()?,
    })
}

pub(crate) fn register_initial_sources(
    data: &Path,
    templates: Option<&Path>,
    fresh: bool,
    seed: impl Fn(&Path, &[u8]) -> Result<(), String>,
) -> Result<Vec<String>, String> {
    let mut baseline = if fresh {
        initial_baseline()?
    } else {
        Baseline::read(data)?
    };
    let (custom_name, custom_bytes) = CUSTOM_TEMPLATE;
    if !fresh
        && baseline.sha256.contains_key(custom_name)
        && (templates.is_none()
            || BUILD_TEMPLATES
                .iter()
                .all(|name| baseline.sha256.contains_key(*name)))
    {
        return Ok(Vec::new());
    }
    let _writer = if fresh {
        None
    } else {
        let setting = data.join("setting");
        fs::create_dir_all(&setting).map_err(message)?;
        let guard = writer(&setting)?;
        // 取得寫入鎖後重新讀取，保留其他程序剛提交的雜湊。
        baseline = Baseline::read(data)?;
        Some(guard)
    };
    let mut sources = Vec::new();
    if !baseline.sha256.contains_key(custom_name) {
        sources.push((custom_name, Cow::Borrowed(custom_bytes)));
    }
    if let Some(templates) = templates {
        for name in BUILD_TEMPLATES {
            if !baseline.sha256.contains_key(name) {
                let bytes = read_table(&templates.join(name))
                    .map_err(|error| format!("無法讀取重建範本 {name}：{error}"))?;
                sources.push((name, Cow::Owned(bytes)));
            }
        }
    }
    let mut notices = Vec::new();
    for (name, bytes) in sources {
        let target = data.join("dictionaries").join(name);
        seed(&target, &bytes)?;
        if !read_table(&target).is_ok_and(|existing| existing == bytes.as_ref()) {
            notices.push(name.to_owned());
        }
        baseline.sha256.insert(name.to_owned(), digest(&bytes)?);
    }
    baseline.write(data)?;
    Ok(notices)
}

fn read_table(path: &Path) -> io::Result<Vec<u8>> {
    let file = File::open(path)?;
    let mut bytes = Vec::new();
    file.take(MAX_BYTES + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "檔案大小超出上限",
        ));
    }
    Ok(bytes)
}

pub(super) fn write_atomic(destination: &Path, contents: &[u8]) -> Result<(), String> {
    crate::project_files::replace_file(destination, contents)
        .map_err(|error| format!("{}：{error}", destination.display()))
}

pub(crate) fn publication_reader(dictionaries: &Path) -> Result<Option<File>, String> {
    let setting = dictionaries
        .parent()
        .ok_or("無法定位設定目錄")?
        .join("setting");
    let guard = match File::open(setting.join(TRANSACTION_LOCK)) {
        Ok(guard) => Some(guard),
        Err(error) if error.kind() == io::ErrorKind::NotFound => None,
        Err(error) => return Err(message(error)),
    };
    if setting.join(PENDING).try_exists().map_err(message)? {
        return Err("字庫更新尚未完成".into());
    }
    Ok(guard)
}

fn writer(setting: &Path) -> Result<File, String> {
    OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .share_mode(0)
        .open(setting.join(TRANSACTION_LOCK))
        .map_err(message)
}

#[derive(Deserialize, Serialize)]
struct Change {
    name: String,
    before: String,
    after: String,
}

fn backup(setting: &Path, index: usize) -> PathBuf {
    setting.join(format!(".dictionary-update-{index}.bak"))
}

fn finish(setting: &Path, count: usize) -> Result<(), String> {
    fs::remove_file(setting.join(PENDING)).map_err(message)?;
    for index in 0..count {
        let _ = fs::remove_file(backup(setting, index));
    }
    Ok(())
}

fn restore(data: &Path, setting: &Path, changes: &[Change]) -> Result<(), String> {
    let dictionaries = data.join("dictionaries");
    for (index, change) in changes.iter().enumerate() {
        let path = dictionaries.join(&change.name);
        match read_table(&path) {
            Ok(bytes) if digest(&bytes)? == change.after => {
                let old = read_table(&backup(setting, index)).map_err(message)?;
                if digest(&old)? != change.before {
                    return Err(format!("復原檔案校驗失敗：{}", change.name));
                }
                write_atomic(&path, &old)?;
            }
            Ok(_) => (), // 現有檔案係舊版，或者使用者已再修改。
            Err(error) if error.kind() == io::ErrorKind::NotFound => (),
            Err(error) => return Err(format!("無法讀取待復原檔案 {}：{error}", change.name)),
        }
    }
    finish(setting, changes.len())
}

pub(super) fn recover(data: &Path, setting: &Path) -> Result<(), String> {
    let pending = match fs::read(setting.join(PENDING)) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(message(error)),
    };
    let _writer = writer(setting)?;
    let changes: Vec<Change> = serde_json::from_slice(&pending).map_err(message)?;
    if changes.is_empty()
        || changes.len() > 2
        || changes
            .iter()
            .any(|change| !TEMPLATES.iter().any(|(name, _)| *name == change.name))
    {
        return Err("字庫更新復原紀錄無效".into());
    }
    let baseline = Baseline::read(data)?;
    if changes
        .iter()
        .all(|change| baseline.sha256.get(&change.name) == Some(&change.after))
    {
        finish(setting, changes.len())
    } else {
        restore(data, setting, &changes)
    }
}

pub(super) enum Failure {
    Changed(Vec<String>),
    Other(String),
}

impl From<String> for Failure {
    fn from(error: String) -> Self {
        Self::Other(error)
    }
}

pub(super) fn read_build_inputs<const N: usize>(
    data: &Path,
    names: [&str; N],
) -> Result<[String; N], Failure> {
    let baseline = Baseline::read(data)?;
    let dictionaries = data.join("dictionaries");
    let mut inputs = [const { String::new() }; N];
    let mut rejected = Vec::new();
    for (name, input) in names.into_iter().zip(&mut inputs) {
        match read_table(&dictionaries.join(name)) {
            Ok(bytes) if baseline.sha256.get(name) == Some(&digest(&bytes)?) => {
                *input = String::from_utf8(bytes)
                    .map_err(|error| format!("重建資料 {name} 編碼無效：{error}"))?;
            }
            Ok(_) => rejected.push(name.to_owned()),
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::NotFound | io::ErrorKind::InvalidData
                ) =>
            {
                rejected.push(name.to_owned());
            }
            Err(error) => return Err(format!("無法核對 {name}：{error}").into()),
        }
    }
    if rejected.is_empty() {
        Ok(inputs)
    } else {
        Err(Failure::Changed(rejected))
    }
}

fn changed_inputs(dictionaries: &Path, inputs: &[(&str, &str)]) -> Vec<String> {
    inputs
        .iter()
        .filter(|(name, expected)| {
            !read_table(&dictionaries.join(name)).is_ok_and(|bytes| bytes == expected.as_bytes())
        })
        .map(|&(name, _)| name.to_owned())
        .collect()
}

// 所有檔案通過 SHA-256 核對後才開始替換。包括組內內容未變的檔案。
pub(super) fn replace(
    data: &Path,
    setting: &Path,
    files: &[(&str, &[u8])],
    inputs: &[(&str, &str)],
) -> Result<bool, Failure> {
    let _writer = writer(setting)?;
    let mut baseline = Baseline::read(data)?;
    let dictionaries = data.join("dictionaries");
    let mut rejected = changed_inputs(&dictionaries, inputs);
    let mut prepared = Vec::new();
    for &(name, new) in files {
        let path = dictionaries.join(name);
        match read_table(&path) {
            Ok(old) => {
                let before = digest(&old)?;
                if baseline.sha256.get(name) != Some(&before) {
                    rejected.push(name.to_owned());
                } else {
                    let change = (old != new)
                        .then(|| -> Result<_, String> {
                            Ok(Change {
                                name: name.into(),
                                before,
                                after: digest(new)?,
                            })
                        })
                        .transpose()?;
                    prepared.push((path, old, new, change));
                }
            }
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::NotFound | io::ErrorKind::InvalidData
                ) =>
            {
                rejected.push(name.to_owned());
            }
            Err(error) => return Err(format!("無法核對 {name}：{error}").into()),
        }
    }
    if !rejected.is_empty() {
        return Err(Failure::Changed(rejected));
    }
    let changes: Vec<_> = prepared
        .iter()
        .filter_map(|(_, _, _, change)| change.as_ref())
        .collect();
    if changes.is_empty() {
        return Ok(false);
    }
    for (index, (_, old, _, _)) in prepared
        .iter()
        .filter(|(_, _, _, change)| change.is_some())
        .enumerate()
    {
        write_atomic(&backup(setting, index), old)?;
    }
    for ((name, _), (path, old, _, _)) in files.iter().zip(&prepared) {
        if !read_table(path).is_ok_and(|current| &current == old) {
            rejected.push((*name).to_owned());
        }
    }
    if !rejected.is_empty() {
        for index in 0..changes.len() {
            let _ = fs::remove_file(backup(setting, index));
        }
        return Err(Failure::Changed(rejected));
    }
    write_atomic(
        &setting.join(PENDING),
        &serde_json::to_vec(&changes).map_err(message)?,
    )?;
    let mut published = Vec::new();
    let result = (|| {
        for (path, old, new, change) in &prepared {
            if let Some(change) = change {
                if !read_table(path).is_ok_and(|bytes| &bytes == old) {
                    return Err(Failure::Changed(vec![change.name.clone()]));
                }
                write_atomic(path, new)?;
                published.push((path, old, new));
                baseline
                    .sha256
                    .insert(change.name.clone(), change.after.clone());
            }
        }
        // 涵蓋沒有內容變動但仍屬於此組的成員，亦保留發佈期間的外部編輯。
        let mut edited = changed_inputs(&dictionaries, inputs);
        edited.extend(files.iter().zip(&prepared).filter_map(
            |(&(name, expected), (path, _, _, _))| match read_table(path) {
                Ok(bytes) if bytes == expected => None,
                _ => Some(name.to_owned()),
            },
        ));
        if !edited.is_empty() {
            return Err(Failure::Changed(edited));
        }
        baseline.write(data)?;
        Ok(())
    })();
    if result.is_err() {
        for (path, old, new) in published.into_iter().rev() {
            if read_table(path).is_ok_and(|bytes| &bytes == new) {
                write_atomic(path, old)?;
            }
        }
    }
    finish(setting, changes.len())?;
    result.map(|()| true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::public_data;

    #[test]
    fn edited_or_missing_build_input_blocks_publication() {
        let data = crate::tests::fixture_dir("protected-build-inputs");
        let setting = data.join("setting");
        let dictionaries = data.join("dictionaries");
        let name = public_data::CUHK_RANK;
        let input = dictionaries.join(name);
        fs::create_dir_all(input.parent().unwrap()).unwrap();
        fs::create_dir(&setting).unwrap();
        let mut baseline = initial_baseline().unwrap();
        baseline
            .sha256
            .insert(name.to_owned(), digest(b"source").unwrap());
        baseline.write(&data).unwrap();
        fs::write(&input, "source").unwrap();
        let rank = dictionaries.join(public_data::RANKING);
        let original = TEMPLATES
            .iter()
            .find(|(name, _)| *name == public_data::RANKING)
            .unwrap()
            .1;
        fs::write(&rank, original).unwrap();
        let [snapshot] = read_build_inputs(&data, [name]).ok().unwrap();
        let files = [(public_data::RANKING, b"new rank".as_slice())];

        fs::write(&input, "edited").unwrap();
        assert!(
            matches!(read_build_inputs(&data, [name]), Err(Failure::Changed(names)) if names == [name])
        );
        assert!(
            matches!(replace(&data, &setting, &files, &[(name, &snapshot)]), Err(Failure::Changed(names)) if names == [name])
        );
        assert_eq!(fs::read(&rank).unwrap(), original);
        fs::remove_file(&input).unwrap();
        assert!(
            matches!(read_build_inputs(&data, [name]), Err(Failure::Changed(names)) if names == [name])
        );
        fs::write(&input, "source").unwrap();
        assert!(matches!(
            replace(&data, &setting, &files, &[(name, &snapshot)]),
            Ok(true)
        ));
        assert_eq!(fs::read(&rank).unwrap(), b"new rank");
        assert_eq!(fs::read(&input).unwrap(), b"source");
        crate::tests::remove_fixture(&data);
    }

    #[test]
    fn cache_retries_the_same_files_after_publication_completes() {
        let data = crate::tests::fixture_dir("dictionary-publication-cache");
        let setting = data.join("setting");
        let dictionaries = data.join("dictionaries");
        fs::create_dir(&setting).unwrap();
        fs::create_dir(&dictionaries).unwrap();
        let path = dictionaries.join(public_data::JYUTPING);
        let mut cache = public_data::Cache::empty();
        let mut load = || {
            cache
                .get(
                    &dictionaries,
                    [public_data::JYUTPING],
                    "jyutping",
                    |[text]| Ok(text.to_owned()),
                )
                .unwrap()
        };

        fs::write(&path, "舊").unwrap();
        let original = load();
        let writer = writer(&setting).unwrap();
        fs::write(setting.join(PENDING), b"[]").unwrap();
        fs::write(&path, "新版").unwrap();
        assert!(std::sync::Arc::ptr_eq(&original, &load()));
        drop(writer);
        assert!(std::sync::Arc::ptr_eq(&original, &load()));
        fs::remove_file(setting.join(PENDING)).unwrap();
        assert_eq!(&*load(), "新版");
        crate::tests::remove_fixture(&data);
    }

    #[test]
    fn preserves_a_modified_pair_and_advances_successful_baselines() {
        let data = crate::tests::fixture_dir("protected-dictionaries");
        let setting = data.join("setting");
        let dictionaries = data.join("dictionaries");
        fs::create_dir_all(dictionaries.join("t2s")).unwrap();
        fs::create_dir(&setting).unwrap();
        for (name, bytes) in TEMPLATES
            .iter()
            .filter(|(name, _)| name.starts_with("t2s/") || *name == public_data::RANKING)
        {
            fs::write(dictionaries.join(name), bytes).unwrap();
        }
        initial_baseline().unwrap().write(&data).unwrap();
        let characters = dictionaries.join(public_data::CHARACTERS);
        let phrases = dictionaries.join(public_data::PHRASES);
        fs::write(&phrases, "自訂").unwrap();
        let group = [
            (public_data::CHARACTERS, &b"new characters"[..]),
            (public_data::PHRASES, &b"new phrases"[..]),
        ];
        assert!(
            matches!(replace(&data, &setting, &group, &[]), Err(Failure::Changed(files)) if files == [public_data::PHRASES])
        );
        assert_eq!(
            fs::read(&characters).unwrap(),
            public_data::INITIAL_CHARACTERS.as_bytes()
        );
        assert_eq!(fs::read_to_string(&phrases).unwrap(), "自訂");
        assert!(matches!(
            replace(&data, &setting, &[(public_data::RANKING, b"new rank")], &[]),
            Ok(true)
        ));
        fs::write(&phrases, public_data::INITIAL_PHRASES).unwrap();
        assert!(matches!(replace(&data, &setting, &group, &[]), Ok(true)));
        assert!(matches!(replace(&data, &setting, &group, &[]), Ok(false)));
        fs::remove_file(&characters).unwrap();
        assert!(
            matches!(replace(&data, &setting, &group, &[]), Err(Failure::Changed(files)) if files == [public_data::CHARACTERS])
        );
        assert!(!characters.exists());
        crate::tests::remove_fixture(&data);
    }

    #[test]
    fn restores_a_pair_when_the_second_replacement_fails_or_a_process_stops() {
        let data = crate::tests::fixture_dir("dictionary-rollback");
        let setting = data.join("setting");
        let dictionaries = data.join("dictionaries");
        fs::create_dir_all(dictionaries.join("t2s")).unwrap();
        fs::create_dir(&setting).unwrap();
        let characters = dictionaries.join(public_data::CHARACTERS);
        let phrases = dictionaries.join(public_data::PHRASES);
        let original = public_data::INITIAL_CHARACTERS.as_bytes();
        let new = b"new characters";
        fs::write(&characters, original).unwrap();
        fs::write(&phrases, public_data::INITIAL_PHRASES).unwrap();
        initial_baseline().unwrap().write(&data).unwrap();
        let reader = OpenOptions::new()
            .read(true)
            .share_mode(1)
            .open(&phrases)
            .unwrap();
        let group = [
            (public_data::CHARACTERS, &new[..]),
            (public_data::PHRASES, &b"new phrases"[..]),
        ];
        assert!(matches!(
            replace(&data, &setting, &group, &[]),
            Err(Failure::Other(_))
        ));
        assert_eq!(fs::read(&characters).unwrap(), original);
        assert_eq!(
            fs::read_to_string(&phrases).unwrap(),
            public_data::INITIAL_PHRASES
        );
        assert!(!setting.join(PENDING).exists());
        drop(reader);

        let changes = [Change {
            name: public_data::CHARACTERS.into(),
            before: digest(original).unwrap(),
            after: digest(new).unwrap(),
        }];
        fs::write(backup(&setting, 0), original).unwrap();
        fs::write(setting.join(PENDING), serde_json::to_vec(&changes).unwrap()).unwrap();
        fs::write(&characters, new).unwrap();
        assert!(publication_reader(&dictionaries).is_err());
        recover(&data, &setting).unwrap();
        assert_eq!(fs::read(&characters).unwrap(), original);
        assert!(publication_reader(&dictionaries).is_ok());
        assert!(matches!(replace(&data, &setting, &group, &[]), Ok(true)));
        crate::tests::remove_fixture(&data);
    }
}
