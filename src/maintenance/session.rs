//! 在登入工作階段啟動字庫服務、定期檢查資料和處理指定程式的前景重啟。
mod programs;

use std::fs;
use std::path::Path;
use std::process::Child;
use std::thread;
use std::time::{Duration, Instant, SystemTime};

use super::{
    broker, broker_command, data, initialise, log_session, project_files, registration,
    require_normal_user, spawn_self, windows,
};

const SETTINGS_POLL: Duration = Duration::from_secs(2);
const UPDATE_POLL: Duration = Duration::from_secs(60);
const FOREGROUND_POLL: Duration = Duration::from_millis(150);

pub(super) fn run(root: &Path) -> Result<(), String> {
    require_normal_user()?;
    let Some(_lock) = windows::Lock::acquire("Session")? else {
        return Ok(());
    };
    initialise(root, data::DEFAULT_POLICY, false, None)?;
    let directory = project_files::user_data(root)?.join("setting");
    if broker::check(None).is_err() {
        broker_command(root)
            .spawn()
            .map_err(|error| error.to_string())?;
    }
    log_session("session_started", true);
    let mut programs = programs::Watch::new();
    let mut next_settings = Instant::now();
    let mut next_update = Instant::now();
    let mut updater: Option<Child> = None;
    loop {
        let now = Instant::now();
        if now >= next_settings {
            programs.reload(&directory);
            next_settings = now + SETTINGS_POLL;
        }
        if let Some(foreground) = programs.ready(now) {
            if registration::profile_is_active() && foreground.is_current() {
                let key = foreground.key;
                let done = restart(root, || {
                    foreground.is_current() && registration::profile_is_active()
                });
                match done {
                    Ok(true) => programs.handled(key),
                    Ok(false) => (),
                    Err(_) => {
                        log_session("auto_restart_failed", false);
                        programs.handled(key);
                    }
                }
            }
        }
        if now >= next_update {
            next_update = now + UPDATE_POLL;
            if updater
                .as_mut()
                .is_some_and(|child| matches!(child.try_wait(), Ok(Some(_)) | Err(_)))
            {
                updater.take();
            }
            if updater.is_none() && update_due(&directory, SystemTime::now()) {
                match spawn_self("--update-scheduled") {
                    Ok(child) => {
                        updater = Some(child);
                        // 如果無法寫下檢查紀錄，也留時間給網路或資料目錄恢復。
                        next_update = now + Duration::from_secs(3600);
                    }
                    Err(_) => log_session("update_start_failed", false),
                }
            }
        }
        thread::sleep(if programs.is_empty() {
            SETTINGS_POLL
        } else {
            FOREGROUND_POLL
        });
    }
}

fn update_due(directory: &Path, now: SystemTime) -> bool {
    let Ok(policy) = fs::read_to_string(directory.join(data::POLICY)) else {
        return false;
    };
    if policy.split_ascii_whitespace().next().is_none() {
        return false;
    }
    let days = fs::read_to_string(directory.join(data::INTERVAL))
        .ok()
        .and_then(|value| value.trim().parse::<u64>().ok())
        .filter(|days| matches!(days, 0 | 1 | 7 | 14 | 30))
        .unwrap_or(7);
    if days == 0 {
        return false;
    }
    let last = [data::REPORT, data::POLICY]
        .into_iter()
        .filter_map(|file| fs::metadata(directory.join(file)).ok()?.modified().ok())
        .next();
    last.and_then(|last| now.duration_since(last).ok())
        .is_some_and(|elapsed| elapsed >= Duration::from_secs(days * 86_400))
}

pub(super) fn restart(root: &Path, still_requested: impl Fn() -> bool) -> Result<bool, String> {
    require_normal_user()?;
    let Some(_lock) = windows::Lock::acquire("Restart")? else {
        return Ok(false);
    };
    if !still_requested() {
        return Ok(false);
    }
    let mut child = broker_command(root)
        .arg("--restart-broker")
        .spawn()
        .map_err(|error| format!("字庫服務未能重新啟動（{error}）"))?;
    let deadline = Instant::now() + Duration::from_secs(10);
    let ready = loop {
        if let Some(status) = child.try_wait().map_err(|error| error.to_string())? {
            return Err(format!("字庫服務未能重新啟動（{status}）"));
        }
        if broker::check(Some(child.id())).is_ok() {
            break true;
        }
        if Instant::now() >= deadline {
            break false;
        }
        thread::sleep(Duration::from_millis(120));
    };
    log_session("broker_restarted", ready);
    if !ready {
        return Err("字庫服務未能重新啟動".into());
    }
    if !still_requested() {
        return Ok(false);
    }
    let run_profile = |operation| {
        spawn_self(operation)?
            .wait()
            .map(|status| status.success())
            .map_err(|error| error.to_string())
    };
    let released = run_profile("--release-profile");
    let activated = run_profile("--activate-profile");
    log_session("profile_restarted", matches!(activated, Ok(true)));
    if !activated? {
        return Err("Windows 未能重新啟用 Oxide IME。可按 Win+Space 選取。".into());
    }
    if !released? {
        return Err("Oxide IME 已啟用。Windows 未能釋放先前的輸入服務。".into());
    }
    Ok(true)
}
