//! 透過使用者資料目錄的設定傳送簡轉繁選詞通知。
use std::fs;
use std::io::{self, Read, Write};
use std::path::Path;
use std::process::Stdio;

use crate::win32::{hidden_command, LocalTime};

use super::files::write_atomic;
use super::message;
use super::s2t::Choices;

const SETTINGS_FILE: &str = "discord-webhook.txt";
const SENT_FILE: &str = ".s2t-discord-sent.json";
const URL_PREFIX: &str = "https://discord.com/api/webhooks/";

fn valid_webhook(url: &str) -> bool {
    url.strip_prefix(URL_PREFIX)
        .and_then(|path| path.split_once('/'))
        .is_some_and(|(id, key)| {
            !id.is_empty()
                && id.len() <= 20
                && id.bytes().all(|b| b.is_ascii_digit())
                && !key.is_empty()
                && key.len() <= 200
                && key
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
        })
}

fn webhook(setting: &Path) -> Result<Option<String>, String> {
    const MAX_BYTES: u64 = 512;
    let file = match fs::File::open(setting.join(SETTINGS_FILE)) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("無法開啟 Discord 設定（{error}）")),
    };
    let mut url = String::new();
    file.take(MAX_BYTES + 1)
        .read_to_string(&mut url)
        .map_err(|error| format!("無法讀取 Discord 設定（{error}）"))?;
    let value = url.trim_start_matches('\u{feff}').trim();
    if url.len() as u64 > MAX_BYTES || (!value.is_empty() && !valid_webhook(value)) {
        return Err("Discord Webhook 網址格式無效".into());
    }
    Ok((!value.is_empty()).then(|| value.to_owned()))
}

fn format_message(timestamp: &str, host: &str, changes: &[(&str, &str)]) -> String {
    let mut result = format!(
        "時間：{timestamp}\n程式：{}\n狀態：成功\n結果：簡轉繁詞組自動採用 {count} 個雙候選詞嘅首選",
        crate::PRODUCT_NAME,
        count = changes.len(),
    );
    let mut units = 0;
    for (index, (word, value)) in changes.iter().enumerate() {
        let item = format!(
            "{}「{word}」→「{value}」",
            if index == 0 { "（" } else { "、" }
        );
        units += item.encode_utf16().count();
        if units > 1500 {
            let remaining = changes.len() - index;
            if index == 0 {
                result.push_str(&format!("（其餘 {remaining} 個）"));
            } else {
                result.push_str(&format!("，另有 {remaining} 個）"));
            }
            break;
        }
        result.push_str(&item);
        if index + 1 == changes.len() {
            result.push('）');
        }
    }
    result.push_str(&format!("\n主機：{host}\n----------------------------"));
    result
}

fn request(curl: &Path, url: &str, body: &str) -> Result<(), String> {
    // URL 由標準輸入傳入，避免將 Webhook 密鑰放入子程序命令列或錯誤紀錄。
    let data = body.replace('\\', "\\\\").replace('"', "\\\"");
    let config = format!("url = \"{url}?wait=true\"\ndata-binary = \"{data}\"\n");
    let mut child = hidden_command(curl)
        .args([
            "--disable",
            "--config",
            "-",
            "--proto",
            "=https",
            "--globoff",
            "--connect-timeout",
            "5",
            "--max-time",
            "15",
            "--silent",
            "--header",
            "Content-Type: application/json",
            "--user-agent",
            "OxideIme-Update/1",
            "--output",
            "NUL",
            "--write-out",
            "%{http_code}",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| "無法啟動 HTTPS 通知")?;
    let written = child
        .stdin
        .take()
        .expect("已建立 stdin 管道")
        .write_all(config.as_bytes());
    let output = child
        .wait_with_output()
        .map_err(|_| "無法取得 HTTPS 通知結果")?;
    written.map_err(|_| "無法寫入 HTTPS 通知")?;
    if !output.status.success() {
        return Err(format!(
            "HTTPS 連線失敗（curl {}）",
            output.status.code().unwrap_or(-1)
        ));
    }
    let status = String::from_utf8_lossy(&output.stdout)
        .parse::<u16>()
        .map_err(|_| "無法辨識 Discord HTTP 狀態")?;
    if !(200..300).contains(&status) {
        return Err(format!("HTTP {status}"));
    }
    Ok(())
}

pub(super) fn notify_s2t(
    setting: &Path,
    curl: &Path,
    choices: &Choices,
) -> Result<Option<usize>, String> {
    let Some(url) = webhook(setting)? else {
        return Ok(None);
    };
    notify_new_choices(&setting.join(SENT_FILE), choices, |changes| {
        let now = LocalTime::now();
        let date = format!(
            "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
            now.year, now.month, now.day, now.hour, now.minute, now.second
        );
        let host = option_env!("OXIDE_IME_DISCORD_HOST")
            .map(str::to_owned)
            .or_else(|| std::env::var("COMPUTERNAME").ok())
            .unwrap_or_else(|| "Windows".into());
        let body = serde_json::json!({
            "content": format_message(&date, &host, changes),
            "allowed_mentions": { "parse": [] },
        });
        request(curl, &url, &body.to_string())?;
        Ok(true)
    })
}

// 只有 Discord 確認接收後才標記新詞組為已通知，失敗時下次檢查會重試。
fn notify_new_choices(
    path: &Path,
    choices: &Choices,
    send: impl FnOnce(&[(&str, &str)]) -> Result<bool, String>,
) -> Result<Option<usize>, String> {
    let known: Choices = match fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes).map_err(message)?,
        Err(error) if error.kind() == io::ErrorKind::NotFound => Choices::new(),
        Err(error) => return Err(message(error)),
    };
    if &known == choices {
        return Ok(None);
    }
    let changes: Vec<_> = choices
        .iter()
        .filter(|(key, choice)| known.get(*key) != Some(*choice))
        .map(|(key, choice)| (key.as_str(), choice.output.as_str()))
        .collect();
    if !changes.is_empty() && !send(&changes)? {
        return Ok(None);
    }
    write_atomic(path, &serde_json::to_vec(choices).map_err(message)?)?;
    Ok((!changes.is_empty()).then_some(changes.len()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_discord_endpoint_and_formats_a_bounded_notification() {
        assert!(valid_webhook(
            "https://discord.com/api/webhooks/123/Example-Token_1"
        ));
        assert!(!valid_webhook(
            "https://discord.com.example/api/webhooks/123/Example-Token_1"
        ));
        assert!(!valid_webhook(
            "https://discord.com/api/webhooks/123/key?wait=true"
        ));
        assert!(!valid_webhook(
            "https://discord.com/api/webhooks/123/key\nurl = other"
        ));
        let result = format_message("2026-11-20 08:00:00", "測試電腦", &[("升平", "昇平")]);
        assert_eq!(result, "時間：2026-11-20 08:00:00\n程式：Oxide IME\n狀態：成功\n結果：簡轉繁詞組自動採用 1 個雙候選詞嘅首選（「升平」→「昇平」）\n主機：測試電腦\n----------------------------");
        let many = vec![("詞組", "首選"); 300];
        let result = format_message("2026-11-20 08:00:00", "測試電腦", &many);
        assert!(result.encode_utf16().count() < 2000);
        assert!(result.contains("另有"));
    }

    #[test]
    fn reads_only_a_valid_configured_endpoint() {
        let setting = crate::tests::fixture_dir("discord-config");
        let path = setting.join(SETTINGS_FILE);
        let url = "https://discord.com/api/webhooks/123/Example-Token_1";
        assert_eq!(webhook(&setting).unwrap(), None);
        fs::write(&path, format!("\u{feff}  {url}\r\n")).unwrap();
        assert_eq!(webhook(&setting).unwrap().as_deref(), Some(url));
        fs::write(&path, " \r\n").unwrap();
        assert_eq!(webhook(&setting).unwrap(), None);
        fs::write(&path, format!("{url}?wait=true")).unwrap();
        assert_eq!(
            webhook(&setting).unwrap_err(),
            "Discord Webhook 網址格式無效"
        );
        fs::write(&path, format!("{url}{}", " ".repeat(512))).unwrap();
        assert!(webhook(&setting).is_err());
        crate::tests::remove_fixture(&setting);
    }

    #[test]
    fn only_repeats_unsent_or_changed_two_candidate_cases() {
        let root = crate::tests::fixture_dir("s2t-discord");
        let state = root.join(SENT_FILE);
        let dictionary = |second: &str| -> Choices {
            serde_json::from_value(serde_json::json!({
                "升平": { "candidates": ["昇平", second], "output": "昇平" },
            }))
            .unwrap()
        };
        let first = dictionary("升平");
        assert_eq!(
            notify_new_choices(&state, &first, |_| Ok(false)).unwrap(),
            None
        );
        assert!(notify_new_choices(&state, &first, |_| Err("中斷".into())).is_err());
        assert!(!state.exists());
        assert_eq!(
            notify_new_choices(&state, &first, |entries| {
                assert_eq!(entries, &[("升平", "昇平")]);
                Ok(true)
            })
            .unwrap(),
            Some(1)
        );
        assert_eq!(
            notify_new_choices(&state, &first, |_| panic!("不應重複發送")).unwrap(),
            None
        );
        assert_eq!(
            notify_new_choices(&state, &dictionary("不同候選"), |_| Ok(true)).unwrap(),
            Some(1)
        );
        assert_eq!(
            notify_new_choices(&state, &Choices::new(), |_| panic!("沒有新詞組")).unwrap(),
            None
        );
        crate::tests::remove_fixture(&root);
    }
}
