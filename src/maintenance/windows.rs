//! 維護程序會用到的 Windows 資源與視窗操作。
use std::ffi::OsString;
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::os::windows::ffi::OsStringExt;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::path::{Path, PathBuf};
use std::ptr;

use crate::{
    tsf::ffi::Raw,
    win32::{wide, wide_os},
};

#[link(name = "kernel32")]
extern "system" {
    fn GetSystemDirectoryW(buffer: *mut u16, size: u32) -> u32;
    fn CreateMutexW(attributes: Raw, initial_owner: i32, name: *const u16) -> Raw;
    fn WaitForSingleObject(handle: Raw, milliseconds: u32) -> u32;
    fn ReleaseMutex(handle: Raw) -> i32;
    fn MoveFileExW(existing: *const u16, new: *const u16, flags: u32) -> i32;
}

#[link(name = "user32")]
extern "system" {
    fn MessageBoxW(owner: Raw, text: *const u16, caption: *const u16, flags: u32) -> i32;
}

pub(super) fn system_directory() -> Result<PathBuf, String> {
    let mut buffer = [0; 1024];
    let length = unsafe { GetSystemDirectoryW(buffer.as_mut_ptr(), buffer.len() as u32) } as usize;
    if length == 0 || length >= buffer.len() {
        return Err("找不到 Windows 系統目錄".into());
    }
    Ok(PathBuf::from(OsString::from_wide(&buffer[..length])))
}

pub(super) fn error_dialog(message: &str) {
    message_dialog(message, 0x10); // MB_ICONERROR
}

pub(super) fn warning_dialog(message: &str) {
    message_dialog(message, 0x30); // MB_ICONWARNING
}

fn message_dialog(message: &str, icon: u32) {
    const MB_TOPMOST: u32 = 0x40000;
    unsafe {
        MessageBoxW(
            ptr::null_mut(),
            wide(message).as_ptr(),
            wide(crate::PRODUCT_NAME).as_ptr(),
            icon | MB_TOPMOST,
        );
    }
}

pub(super) struct Lock {
    handle: OwnedHandle,
}

impl Lock {
    pub(super) fn acquire(operation: &str) -> Result<Option<Self>, String> {
        Self::acquire_for(operation, 0)
    }

    pub(super) fn acquire_for(operation: &str, timeout_ms: u32) -> Result<Option<Self>, String> {
        let name = format!(
            r"Local\OxideIme.{operation}.{}",
            crate::broker::current_logon_sid()?
        );
        let raw = unsafe { CreateMutexW(ptr::null_mut(), 0, wide(&name).as_ptr()) };
        if raw.is_null() {
            return Err(io::Error::last_os_error().to_string());
        }
        let handle = unsafe { OwnedHandle::from_raw_handle(raw) };
        match unsafe { WaitForSingleObject(handle.as_raw_handle(), timeout_ms) } {
            0 | 0x80 => Ok(Some(Self { handle })),
            0x102 => Ok(None),
            _ => Err(io::Error::last_os_error().to_string()),
        }
    }
}

impl Drop for Lock {
    fn drop(&mut self) {
        unsafe { ReleaseMutex(self.handle.as_raw_handle()) };
    }
}

pub(super) fn publish_initial(stage: &Path, destination: &Path) -> io::Result<()> {
    let from = wide_os(stage.as_os_str());
    let to = wide_os(destination.as_os_str());
    if unsafe { MoveFileExW(from.as_ptr(), to.as_ptr(), 8) } != 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

pub(super) struct ManifestFile {
    path: PathBuf,
}

impl ManifestFile {
    pub(super) fn create() -> Result<Self, String> {
        // 管理員的暫存資源留在受保護的 Windows 安裝目錄。
        let path = std::env::current_exe()
            .map_err(|error| error.to_string())?
            .with_file_name(format!(".oxide-ime-events-{}.man", std::process::id()));
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .map_err(|error| error.to_string())?;
        let manifest = Self { path };
        file.write_all(include_bytes!("../../assets/oxide-ime-events.man"))
            .and_then(|()| file.sync_all())
            .map_err(|error| error.to_string())?;
        Ok(manifest)
    }

    pub(super) fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for ManifestFile {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

fn binary_release(name: &str) -> Option<&str> {
    let release = name
        .strip_prefix("oxide_ime_broker_")
        .and_then(|name| name.strip_suffix(".exe"))
        .or_else(|| {
            name.strip_prefix("oxide_ime_maintenance_")
                .and_then(|name| name.strip_suffix(".exe"))
        })
        .or_else(|| {
            let version = name.strip_prefix("oxide_ime_")?.strip_suffix(".dll")?;
            Some(version.strip_suffix("_x86").unwrap_or(version))
        })?;
    let mut parts = release.strip_prefix('v')?.split('.');
    let major = parts.next()?;
    let minor = parts.next()?;
    let patch = parts.next();
    let numeric = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    (!major.starts_with('0')
        && numeric(major)
        && numeric(minor)
        && patch.is_none_or(numeric)
        && parts.next().is_none())
    .then_some(release)
}

pub(super) fn clean_binaries(root: &Path, current: &str) -> Result<bool, String> {
    let mut reboot = false;
    for entry in fs::read_dir(root.join("bin")).map_err(|error| error.to_string())? {
        let entry = entry.map_err(|error| error.to_string())?;
        let name = entry.file_name().to_string_lossy().to_ascii_lowercase();
        if !entry
            .file_type()
            .map_err(|error| error.to_string())?
            .is_file()
            || binary_release(&name).is_none_or(|release| release == current)
        {
            continue;
        }
        let path = entry.path();
        if let Err(error) = fs::remove_file(&path) {
            if error.kind() == io::ErrorKind::NotFound {
                continue;
            }
            let path = wide_os(path.as_os_str());
            const MOVEFILE_DELAY_UNTIL_REBOOT: u32 = 4;
            if unsafe { MoveFileExW(path.as_ptr(), ptr::null(), MOVEFILE_DELAY_UNTIL_REBOOT) } == 0
            {
                return Err(format!("無法清理舊版程式 {name}（{error}）"));
            }
            reboot = true;
        }
    }
    Ok(reboot)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn upgrade_removes_only_superseded_product_binaries() {
        let root = crate::tests::fixture_dir("upgrade-binaries");
        let bin = root.join("bin");
        fs::create_dir(&bin).unwrap();
        for file in [
            "oxide_ime_v1.0.dll",
            "oxide_ime_v1.0_x86.dll",
            "oxide_ime_broker_v1.0.exe",
            "oxide_ime_maintenance_v1.0.exe",
            "oxide_ime_v1.1.dll",
            "unrelated_v1.0.dll",
        ] {
            fs::write(bin.join(file), b"fixture").unwrap();
        }
        assert!(!clean_binaries(&root, "v1.1").unwrap());
        let mut files = fs::read_dir(bin)
            .unwrap()
            .map(|file| file.unwrap().file_name().into_string().unwrap())
            .collect::<Vec<_>>();
        files.sort();
        assert_eq!(files, ["oxide_ime_v1.1.dll", "unrelated_v1.0.dll"]);
        crate::tests::remove_fixture(&root);
    }
}
