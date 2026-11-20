//! 維護程式的命令入口。登入工作與互動視窗各自在獨立程序執行。
pub(crate) mod caret;
mod data;
mod session;
mod windows;

use std::ffi::OsStr;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

use crate::diagnostics::{self, Level};
use crate::tsf::registration;
use crate::win32::{allow_foreground, hidden_command};
use crate::{broker, project_files, update};

const RELEASE: &str = diagnostics::RELEASE.trim_ascii();

pub(super) fn run() -> Result<(), String> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    let root = project_files::executable_root()?;
    match args.as_slice() {
        [command] if command == "--session" => session::run(&root),
        [command] if command == "--correct-caret" => caret::run(),
        [command] if command == "--update-public" => {
            require_normal_user()?;
            update::run(&root, |_| {})
        }
        [command] if command == "--update-scheduled" => {
            require_normal_user()?;
            update::run(&root, windows::warning_dialog)
        }
        [command, csv, words, output] if command == "--rebuild-jyutping" => {
            require_normal_user()?;
            crate::update::rebuild_jyutping(Path::new(csv), Path::new(words), Path::new(output))
        }
        [command, folder] if command == "--stage-cangjie" => {
            require_normal_user()?;
            crate::update::stage_cangjie(Path::new(folder))
        }
        [command, updates, templates, rest @ ..] if command == "--initialize" => {
            require_normal_user()?;
            let updates = updates.to_str().ok_or("無效的公用資料選項")?;
            let choose = match rest {
                [] => false,
                [option] if option == "--select-data" => true,
                _ => return Err("無效的初始化參數".into()),
            };
            initialise(&root, updates, choose, Some(Path::new(templates)))
        }
        [command] if command == "--release-profile" => change_input_profile(false),
        [command] if command == "--activate-profile" => change_input_profile(true),
        [command]
            if ["--register", "--prepare-upgrade", "--unregister"]
                .iter()
                .any(|arg| command == arg) =>
        {
            admin(&root, command)
        }
        [command, action, release, architecture] if command == "--action" => {
            if !matches!(architecture.to_str(), Some("x64" | "x86")) {
                return Err("無效的輸入法架構".into());
            }
            let result = match action.to_str() {
                Some("Restart") => session::restart(&root, || true).map(|_| ()),
                #[cfg(feature = "private-discord")]
                Some("DiscordSettings") => dialog(&root, action, release, architecture).map(|_| ()),
                Some(
                    "OpenData" | "OpenProgram" | "CheckUpdates" | "UpdateSettings" | "ExportLogs"
                    | "Diagnostics" | "ClearLogs" | "About" | "RestartSettings",
                ) => dialog(&root, action, release, architecture).map(|_| ()),
                _ => Err("無效的維護選項".into()),
            };
            if let Err(error) = &result {
                windows::error_dialog(error);
            }
            result
        }
        _ => Err("無效的維護參數".into()),
    }
}

fn change_input_profile(activate: bool) -> Result<(), String> {
    let operation = if activate { "啟用" } else { "釋放" };
    registration::change_profile(activate)
        .map_err(|hr| format!("無法{operation}輸入法（0x{:08X}）", hr as u32))
}

fn require_normal_user() -> Result<(), String> {
    if broker::is_elevated()? {
        Err("請從一般權限的程式執行這項使用者操作。".into())
    } else {
        Ok(())
    }
}

fn initialise(
    root: &Path,
    updates: &str,
    choose: bool,
    templates: Option<&Path>,
) -> Result<(), String> {
    if !project_files::installed(root) {
        return Err("初始化需要正式安裝版".into());
    }
    let default_directory = project_files::user_data(root)?;
    let chosen = if choose {
        let answer = dialog(
            root,
            OsStr::new("ChooseData"),
            OsStr::new(RELEASE),
            OsStr::new("x64"),
        )?;
        let path = answer.trim_end_matches(['\r', '\n']);
        (!path.is_empty()).then(|| PathBuf::from(path))
    } else {
        None
    };
    let directory = chosen.as_deref().unwrap_or(&default_directory);
    if !directory.is_absolute() {
        return Err("請選擇完整的資料目錄路徑".into());
    }
    let notices = data::initialise(
        directory,
        &project_files::user_root()?.join("docs"),
        templates,
        updates,
    )?;
    if templates.is_some() && !notices.is_empty() {
        windows::warning_dialog(&format!(
            "已保留資料目錄中修改過嘅檔案：\n{}",
            notices.join("、")
        ));
    }
    if chosen.is_some() {
        project_files::configure_user_data(directory)?;
    }
    Ok(())
}

fn broker_command(root: &Path) -> Command {
    hidden_command(project_files::binaries(root).join(format!("oxide_ime_broker_{RELEASE}.exe")))
}

fn spawn_self(argument: &str) -> Result<Child, String> {
    hidden_command(std::env::current_exe().map_err(|error| error.to_string())?)
        .arg(argument)
        .spawn()
        .map_err(|error| error.to_string())
}

const DEFAULT_RESTART_PROGRAMS: &str = r#"[{"path":"League of Legends.exe","delay_ms":1500}]"#;

fn dialog(root: &Path, action: &OsStr, release: &OsStr, arch: &OsStr) -> Result<String, String> {
    let data = project_files::user_data(root)?;
    let source = concat!(
        include_str!("maintenance/events.ps1"),
        "\n",
        include_str!("maintenance/dialog-support.ps1"),
        "\n",
        include_str!("maintenance/dialogs.ps1")
    );
    #[cfg(feature = "private-discord")]
    let source = if action == "DiscordSettings" {
        concat!(
            include_str!("maintenance/dialog-support.ps1"),
            "\n",
            include_str!("maintenance/discord-settings.ps1")
        )
    } else {
        source
    };
    powershell(
        root,
        source,
        &[
            ("OXIDE_IME_ACTION", action),
            (
                "OXIDE_IME_RESTART_DEFAULT",
                OsStr::new(DEFAULT_RESTART_PROGRAMS),
            ),
            ("OXIDE_IME_LOADED", release),
            ("OXIDE_IME_ARCH", arch),
            ("OXIDE_IME_DATA", data.as_os_str()),
        ],
    )
}

fn admin(root: &Path, action: &OsStr) -> Result<(), String> {
    if !broker::is_elevated()? {
        return Err("這項操作需要管理員權限".into());
    }
    // 事件資訊清單必須交給 Windows 登錄工具。實際資源在本 EXE，資訊清單用完即釋放。
    let workspace = windows::ManifestFile::create()?;
    let output = powershell(
        root,
        concat!(
            include_str!("maintenance/events.ps1"),
            "\n",
            include_str!("maintenance/registration.ps1")
        ),
        &[
            ("OXIDE_IME_ACTION", action),
            ("OXIDE_IME_MANIFEST", workspace.path().as_os_str()),
            ("OXIDE_IME_BIN", project_files::binaries(root).as_os_str()),
        ],
    )?;
    if !output.trim().is_empty() {
        print!("{output}");
    }
    if action == "--register"
        && project_files::installed(root)
        && windows::clean_binaries(root, RELEASE)?
    {
        println!("OXIDE_IME_RESTART_REQUIRED");
    }
    Ok(())
}

fn powershell(root: &Path, source: &str, variables: &[(&str, &OsStr)]) -> Result<String, String> {
    // 固定 Windows 內建執行檔及固定命令。已編譯指令碼從管道傳送，路徑只當資料使用。
    let system = windows::system_directory()?;
    let executable = std::env::current_exe().map_err(|error| error.to_string())?;
    let mut child = hidden_command(system.join("WindowsPowerShell/v1.0/powershell.exe"))
        .args([
            "-NoLogo", "-NoProfile", "-STA", "-NonInteractive", "-Command",
            "$ErrorActionPreference='Stop'; [Console]::InputEncoding=[Console]::OutputEncoding=[Text.UTF8Encoding]::new($false); try { & ([scriptblock]::Create([Console]::In.ReadToEnd())) } catch { [Console]::Error.WriteLine($_.Exception.Message); exit 1 }",
        ])
        .env("OXIDE_IME_ROOT", root)
        .env("OXIDE_IME_EXE", executable)
        .env("OXIDE_IME_RELEASE", RELEASE)
        .env("OXIDE_IME_PARENT_PID", std::process::id().to_string())
        .env("OXIDE_IME_INSTALLED", if project_files::installed(root) { "1" } else { "0" })
        .envs(variables.iter().copied())
        .current_dir(root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| error.to_string())?;
    allow_foreground(child.id());
    let input = child
        .stdin
        .take()
        .ok_or("無法開啟維護命令管道")?
        .write_all(source.as_bytes());
    let output = child
        .wait_with_output()
        .map_err(|error| error.to_string())?;
    if output.status.success() {
        input.map_err(|error| error.to_string())?;
        String::from_utf8(output.stdout).map_err(|error| error.to_string())
    } else {
        let error = String::from_utf8_lossy(&output.stderr);
        Err(if error.trim().is_empty() {
            format!("Windows 維護命令未完成（{}）", output.status)
        } else {
            error.trim().to_owned()
        })
    }
}

fn log_session(operation: &'static str, ok: bool) {
    diagnostics::event(
        if ok { Level::Info } else { Level::Warning },
        "maintenance",
        operation,
        &[("ok", i64::from(ok))],
    );
}
