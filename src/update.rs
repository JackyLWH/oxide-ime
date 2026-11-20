//! 由維護程序以登入者權限同步已選擇的公開來源。
#[cfg(feature = "private-discord")]
mod discord;
mod files;
mod jyutping;
mod ranking;
mod s2t;

use std::collections::HashSet;
use std::fs::{self, File, OpenOptions};
use std::os::windows::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use crate::diagnostics::{self, Level};
use crate::dictionary::Dictionary;
use crate::public_data;

use crate::simplify::Converter;
use crate::win32::hidden_command;
pub(crate) use files::{publication_reader, register_initial_sources};
use files::{write_atomic, Failure};

const OPENCC: &str = "BYVoid/OpenCC";
const RIME: &str = "rime/rime-cantonese";
const CANCLID: &str = "CanCLID/rime-cantonese-upstream";
const CANGJIE: &str = "Arthurmcarthur/Cangjie3-Plus";

fn message(error: impl std::fmt::Display) -> String {
    error.to_string()
}

struct Download {
    folder: PathBuf,
    curl: PathBuf,
}

impl Download {
    fn new(folder: &Path) -> Result<Self, String> {
        let windows = std::env::var_os("SystemRoot")
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
            .ok_or("找不到 Windows 目錄")?;
        Ok(Self {
            folder: folder.to_owned(),
            curl: windows.join("System32/curl.exe"),
        })
    }

    fn get(&self, url: &str) -> Result<String, String> {
        let path = self.folder.join(".download");
        let result = (|| {
            let output = hidden_command(&self.curl)
                .args([
                    "--disable",
                    "--fail",
                    "--silent",
                    "--show-error",
                    "--location",
                    "--max-redirs",
                    "3",
                    "--proto",
                    "=https",
                    "--proto-redir",
                    "=https",
                    "--connect-timeout",
                    "10",
                    "--max-time",
                    "45",
                    "--retry",
                    "2",
                    "--retry-delay",
                    "1",
                    "--retry-max-time",
                    "50",
                    "--max-filesize",
                ])
                .arg(public_data::MAX_BYTES.to_string())
                .arg("--output")
                .arg(&path)
                .args(["--user-agent", "OxideIme-PublicData/1", "--", url])
                .stdout(Stdio::null())
                .stderr(Stdio::piped())
                .output()
                .map_err(message)?;
            if !output.status.success() {
                diagnostics::event(
                    Level::Warning,
                    "update",
                    "download_error",
                    &[("curl_exit", i64::from(output.status.code().unwrap_or(-1)))],
                );
                return Err(format!(
                    "{url}：{}",
                    String::from_utf8_lossy(&output.stderr).trim()
                ));
            }
            if fs::metadata(&path).map_err(message)?.len() > public_data::MAX_BYTES {
                return Err(format!("{url} 超過公用資料大小上限"));
            }
            fs::read_to_string(&path).map_err(message)
        })();
        let _ = fs::remove_file(path);
        result
    }

    fn latest(&self, repo: &'static str, branch: &str) -> Result<Revision<'_>, String> {
        let url = format!("https://api.github.com/repos/{repo}/commits/{branch}");
        let source = self.get(&url)?;
        let json: serde_json::Value = serde_json::from_str(&source).map_err(message)?;
        let sha = json["sha"]
            .as_str()
            .filter(|value| value.len() == 40 && value.bytes().all(|ch| ch.is_ascii_hexdigit()))
            .ok_or("GitHub 修訂編號無效")?;
        Ok(Revision {
            download: self,
            repo,
            sha: sha.to_owned(),
        })
    }
}

struct Revision<'a> {
    download: &'a Download,
    repo: &'static str,
    sha: String,
}

impl Revision<'_> {
    fn file(&self, path: &str) -> Result<String, String> {
        self.download.get(&format!(
            "https://raw.githubusercontent.com/{}/{}/{path}",
            self.repo, self.sha,
        ))
    }
}

fn exclusive(folder: &Path) -> Result<File, String> {
    fs::create_dir_all(folder).map_err(message)?;
    OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(false)
        .share_mode(0)
        .open(folder.join(".update-lock"))
        .map_err(|error| format!("無法取得公用資料更新鎖：{error}"))
}

struct Update<'a> {
    data: &'a Path,
    setting: &'a Path,
    source: Download,
}

impl Update<'_> {
    fn publish(&self, files: &[(&str, &[u8])]) -> Result<bool, Failure> {
        files::replace(self.data, self.setting, files, &[])
    }
}

fn opencc(update: &Update<'_>) -> Result<String, Failure> {
    let source = update.source.latest(OPENCC, "master")?;
    let characters = source.file("data/dictionary/TSCharacters.txt")?;
    let phrases = source.file("data/dictionary/TSPhrases.txt")?;
    Converter::new(&characters, &phrases)?;
    let changed = update.publish(&[
        (public_data::CHARACTERS, characters.as_bytes()),
        (public_data::PHRASES, phrases.as_bytes()),
    ])?;
    Ok(status(changed, &source.sha))
}

fn s2t(update: &Update<'_>) -> Result<String, Failure> {
    let rules = files::read_build_inputs(update.data, public_data::S2T_RULES)?;
    let source = update.source.latest(OPENCC, "master")?;
    let build = s2t::build(
        &source.file("data/dictionary/STPhrases.txt")?,
        rules.each_ref().map(String::as_str),
    )?;
    let inputs: [_; 3] = std::array::from_fn(|i| (public_data::S2T_RULES[i], rules[i].as_str()));
    let changed = files::replace(
        update.data,
        update.setting,
        &[(public_data::S2T_PHRASES, build.json.as_bytes())],
        &inputs,
    )?;
    let report = status(changed, &source.sha);
    #[cfg(feature = "private-discord")]
    let report = {
        let mut report = report;
        let notice = discord::notify_s2t(update.setting, &update.source.curl, &build.choices);
        match notice {
            Ok(Some(count)) => {
                report.push_str(&format!("。已向 Discord 通知 {count} 個雙候選詞組"))
            }
            Ok(None) => (),
            Err(error) => {
                diagnostics::event(Level::Warning, "update", "s2t_discord_notify_failed", &[]);
                report.push_str(&format!("。Discord 通知未完成（{error}）"));
            }
        }
        report
    };
    Ok(report)
}

fn jyutping(update: &Update<'_>) -> Result<String, Failure> {
    let rime = update.source.latest(RIME, "main")?;
    let canclid = update.source.latest(CANCLID, "main")?;
    let contents = jyutping::build(
        &canclid.file("char.csv")?,
        &rime.file("jyut6ping3.words.dict.yaml")?,
    )?;
    let changed = update.publish(&[(public_data::JYUTPING, contents.as_bytes())])?;
    Ok(status(
        changed,
        &format!("Rime {}，CanCLID {}", rime.sha, canclid.sha),
    ))
}

pub(super) fn rebuild_jyutping(csv: &Path, words: &Path, destination: &Path) -> Result<(), String> {
    let read = |path: &Path| fs::read_to_string(path).map_err(message);
    let contents = jyutping::build(&read(csv)?, &read(words)?)?;
    // 開發用匯出只接受新路徑。正式更新由資料保護流程發佈。
    use std::io::Write as _;
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(destination)
        .map_err(message)?
        .write_all(contents.as_bytes())
        .map_err(message)
}

fn cangjie_yaml(source: &str, sha: &str) -> Result<String, String> {
    let mut lines = source.trim_start_matches('\u{feff}').lines();
    if lines.next() != Some("encode=UTF-8") {
        return Err("Cangjie3-Plus 碼表編碼不符".into());
    }
    let mut lines = lines.skip_while(|line| *line != "[DATA]");
    if lines.next().is_none() {
        return Err("Cangjie3-Plus 缺少碼表正文".into());
    }
    let mut yaml =
        format!("# Cangjie3-Plus，來源修訂 {sha}\n---\nname: cangjie3\nsort: original\n...\n");
    let mut count = 0;
    for line in lines.filter(|line| !line.is_empty()) {
        let (code, text) = line
            .split_once([' ', '\t'])
            .ok_or("Cangjie3-Plus 缺少字詞")?;
        // 全形空格也是一個字詞。
        let text = text.trim_matches(|ch: char| ch.is_ascii_whitespace());
        if code.is_empty()
            || code.len() > 6
            || !code.bytes().all(|b| b.is_ascii_lowercase())
            || text.is_empty()
            || text.contains(['\t', '\0'])
        {
            return Err("Cangjie3-Plus 碼表有無效詞條".into());
        }
        yaml.push_str(text);
        yaml.push('\t');
        yaml.push_str(code);
        yaml.push('\n');
        count += 1;
    }
    if count < 70_000 {
        return Err("Cangjie3-Plus 碼表不完整".into());
    }
    Dictionary::parse([(public_data::CANGJIE, yaml.as_str())])?;
    Ok(yaml)
}

fn cangjie(update: &Update<'_>) -> Result<String, Failure> {
    let source = update.source.latest(CANGJIE, "master")?;
    let contents = cangjie_yaml(&source.file("cj3.txt")?, &source.sha)?;
    let changed = update.publish(&[(public_data::CANGJIE, contents.as_bytes())])?;
    Ok(status(changed, &source.sha))
}

pub(super) fn stage_cangjie(folder: &Path) -> Result<(), String> {
    let _lock = exclusive(folder)?;
    let download = Download::new(folder)?;
    let source = download.latest(CANGJIE, "master")?;
    let contents = cangjie_yaml(&source.file("cj3.txt")?, &source.sha)?;
    let license = source.file("LICENSE")?;
    if !license.starts_with("MIT License") {
        return Err("Cangjie3-Plus 授權檔案與預期不符".into());
    }
    write_atomic(&folder.join("LICENSE.txt"), license.as_bytes())?;
    write_atomic(&folder.join(public_data::CANGJIE), contents.as_bytes())?;
    Ok(())
}

fn status(changed: bool, revision: &str) -> String {
    let verb = if changed { "已套用" } else { "已是最新" };
    format!("{verb}（{revision}）")
}

pub(super) fn run(root: &Path, notify: impl FnOnce(&str)) -> Result<(), String> {
    let result = update_enabled_groups(root, notify);
    diagnostics::event(
        if result.is_ok() {
            diagnostics::Level::Info
        } else {
            diagnostics::Level::Error
        },
        "update",
        "run",
        &[("ok", i64::from(result.is_ok()))],
    );
    result
}

fn update_enabled_groups(root: &Path, notify: impl FnOnce(&str)) -> Result<(), String> {
    type Refresh = fn(&Update<'_>) -> Result<String, Failure>;
    let groups: [(&str, &str, Refresh); 5] = [
        ("opencc", "繁轉簡", opencc),
        ("s2t", "簡轉繁詞組", s2t),
        ("jyutping", "粵拼", jyutping),
        ("ranking", "常用字及字頻", ranking::update),
        ("cangjie", "倉頡", cangjie),
    ];
    let data = crate::project_files::user_data(root)?;
    let setting = data.join("setting");
    let config = match fs::read_to_string(setting.join("update-policy.txt")) {
        Ok(config) => config,
        Err(error)
            if error.kind() == std::io::ErrorKind::NotFound
                && !crate::project_files::installed(root) =>
        {
            "opencc\ns2t\njyutping\nranking".into()
        }
        Err(error) => return Err(message(error)),
    };
    let active: HashSet<&str> = config.split_ascii_whitespace().collect();
    if !active
        .iter()
        .all(|key| groups.iter().any(|group| group.0 == *key))
    {
        return Err("公用資料更新選項無效".into());
    }
    let _lock = exclusive(&setting)?;
    files::recover(&data, &setting)?;
    let context = Update {
        data: &data,
        setting: &setting,
        source: Download::new(&setting)?,
    };
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(message)?
        .as_secs();
    let mut log = format!("UTC Unix 時間 {timestamp}\n");
    let mut failed = false;
    let mut changed_files = Vec::new();
    for (key, label, update) in groups {
        let started = Instant::now();
        let mut applied = true;
        let result = if active.contains(key) {
            match update(&context) {
                Ok(result) => result,
                Err(Failure::Other(error)) => {
                    failed = true;
                    applied = false;
                    format!("沿用上一版（{error}）")
                }
                Err(Failure::Changed(files)) => {
                    failed = true;
                    applied = false;
                    let summary = format!("{label}：{}", files.join("、"));
                    changed_files.push(summary);
                    format!("已取消此組更新，檔案已更改或刪除：{}", files.join("、"))
                }
            }
        } else {
            "已停用更新".into()
        };
        diagnostics::event(
            if applied { Level::Info } else { Level::Error },
            "update",
            key,
            &[
                ("enabled", i64::from(active.contains(key))),
                ("ok", i64::from(applied)),
                ("elapsed_ms", diagnostics::millis(started.elapsed())),
            ],
        );
        log.push_str(&format!("{label}　{result}\n"));
    }
    write_atomic(&setting.join("last-update.txt"), log.as_bytes())?;
    drop(_lock);
    if !changed_files.is_empty() {
        notify(&format!(
            "已保留你更改或刪除嘅詞庫，並取消以下組別嘅更新：\n{}",
            changed_files.join("\n")
        ));
    }
    if failed {
        Err("部分公用資料更新失敗，詳見 last-update.txt".into())
    } else {
        Ok(())
    }
}
