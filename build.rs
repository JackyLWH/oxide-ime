//! 為輸入法 DLL 編譯圖示，為維護程式編譯 Windows 事件資源。
use std::path::{Path, PathBuf};
use std::process::Command;

fn sdk_resource_compiler() -> Option<PathBuf> {
    if let (Some(root), Some(version)) = (
        std::env::var_os("WindowsSdkDir"),
        std::env::var_os("WindowsSDKVersion"),
    ) {
        let candidate = Path::new(&root)
            .join("bin")
            .join(version)
            .join("x64")
            .join("rc.exe");
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    let root = PathBuf::from(std::env::var_os("ProgramFiles(x86)")?).join("Windows Kits/10/bin");
    std::fs::read_dir(root)
        .ok()?
        .filter_map(Result::ok)
        .map(|entry| entry.path().join("x64/rc.exe"))
        .filter(|path| path.is_file())
        .max()
}

fn compile_resource(rc: &Path, source: &Path, output: &Path, directory: &Path) {
    let status = Command::new(rc)
        .current_dir(directory)
        .arg("/nologo")
        .arg(format!("/fo{}", output.display()))
        .arg(source)
        .status()
        .unwrap_or_else(|error| panic!("無法執行 Windows SDK 資源編譯器：{error}"));
    assert!(status.success(), "編譯資源 {} 失敗", source.display());
}

fn version_resource(rc: &Path, out: &Path) -> PathBuf {
    let release = std::env::var("RUST_IME_RELEASE")
        .unwrap_or_else(|_| include_str!("release-version.txt").trim().to_owned());
    let mut parts = release
        .strip_prefix('v')
        .expect("無效的發行版本")
        .split('.');
    let mut version = [0u16; 3];
    for part in &mut version[..2] {
        *part = parts
            .next()
            .and_then(|value| value.parse().ok())
            .expect("無效的發行版本");
    }
    if let Some(patch) = parts.next() {
        version[2] = patch.parse().expect("無效的發行版本");
    }
    assert!(parts.next().is_none() && version[0] > 0, "無效的發行版本");
    let [major, minor, patch] = version;
    let source = out.join("oxide-ime-version.rc");
    std::fs::write(
        &source,
        format!(
            r#"
1 VERSIONINFO
 FILEVERSION {major},{minor},{patch},0
 PRODUCTVERSION {major},{minor},{patch},0
 FILEOS 0x40004
 FILETYPE 0
BEGIN
 BLOCK "StringFileInfo"
 BEGIN
  BLOCK "040904b0"
  BEGIN
   VALUE "FileDescription", "Oxide IME"
   VALUE "ProductName", "Oxide IME"
   VALUE "FileVersion", "{major}.{minor}.{patch}.0"
   VALUE "ProductVersion", "{release}"
  END
 END
 BLOCK "VarFileInfo"
 BEGIN
  VALUE "Translation", 0x0409, 0x04b0
 END
END
"#
        ),
    )
    .expect("無法寫入版本資源");
    let output = out.join("oxide-ime-version.res");
    compile_resource(rc, &source, &output, out);
    output
}

fn main() {
    println!("cargo:rerun-if-changed=assets/oxide-ime.rc");
    println!("cargo:rerun-if-changed=assets/oxide-ime.ico");
    println!("cargo:rerun-if-changed=assets/oxide-ime.def");
    println!("cargo:rerun-if-changed=assets/oxide-ime-events.man");
    println!("cargo:rerun-if-changed=release-version.txt");
    for variable in [
        "RUST_IME_RELEASE",
        "WindowsSdkDir",
        "WindowsSDKVersion",
        "ProgramFiles(x86)",
    ] {
        println!("cargo:rerun-if-env-changed={variable}");
    }
    let project = Path::new(env!("CARGO_MANIFEST_DIR"));
    let rc = sdk_resource_compiler().expect("找不到 Windows SDK x64 rc.exe");
    let out = PathBuf::from(std::env::var_os("OUT_DIR").unwrap());
    let output = out.join("oxide-ime.res");
    compile_resource(&rc, Path::new("assets/oxide-ime.rc"), &output, project);
    println!("cargo:rustc-link-arg-cdylib={}", output.display());
    let version = version_resource(&rc, &out);
    println!("cargo:rustc-link-arg-cdylib={}", version.display());
    for app in ["oxide-ime-broker", "oxide-ime-maintenance"] {
        println!("cargo:rustc-link-arg-bin={app}={}", version.display());
    }

    let manifest = project.join("assets/oxide-ime-events.man");
    let status = Command::new(rc.with_file_name("mc.exe"))
        .current_dir(&out)
        .args(["-um", "-z", "oxide-ime-events"])
        .arg(manifest)
        .status()
        .expect("無法執行 Windows SDK 事件訊息編譯器");
    assert!(status.success(), "編譯事件資訊清單失敗");
    let output = out.join("oxide-ime-events.res");
    compile_resource(&rc, Path::new("oxide-ime-events.rc"), &output, &out);
    println!(
        "cargo:rustc-link-arg-bin=oxide-ime-maintenance={}",
        output.display()
    );
    // COM 入口只供動態載入，毋須產生匯入程式庫。
    println!("cargo:rustc-link-arg-cdylib=/NOIMPLIB");
    let exports = project.join("assets/oxide-ime.def");
    println!("cargo:rustc-link-arg-cdylib=/DEF:{}", exports.display());
}
