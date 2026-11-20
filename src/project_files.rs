//! 已安裝的 DLL、字庫服務及資料目錄共用的路徑規則。
use crate::win32::{registry, wide};
use std::ffi::OsString;
use std::os::windows::ffi::OsStringExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::OnceLock;
use std::time::SystemTime;
use std::{
    fs,
    io::{self, Write},
};

use crate::tsf::ffi::{self, Guid, HResult, Raw};
use crate::tsf::MODULE;

pub(crate) fn dll_path() -> Result<String, HResult> {
    let module = MODULE.load(Ordering::Relaxed);
    if module.is_null() {
        return Err(ffi::E_UNEXPECTED);
    }
    let mut path = vec![0u16; 32768];
    let len = unsafe { ffi::GetModuleFileNameW(module, path.as_mut_ptr(), path.len() as u32) };
    let len = len as usize;
    if len == 0 || len >= path.len() {
        return Err(ffi::E_FAIL);
    }
    String::from_utf16(&path[..len]).map_err(|_| ffi::E_FAIL)
}

fn root_from_binary(path: &Path) -> Result<PathBuf, String> {
    path.parent()
        .and_then(Path::parent)
        .map(Path::to_path_buf)
        .ok_or_else(|| "無法定位輸入法專案".into())
}

pub(crate) fn dll_root() -> Result<PathBuf, String> {
    let path = dll_path().map_err(|_| "無法取得 DLL 路徑".to_owned())?;
    root_from_binary(Path::new(&path))
}

pub(crate) fn executable_root() -> Result<PathBuf, String> {
    let executable = std::env::current_exe().map_err(|error| error.to_string())?;
    root_from_binary(&executable)
}

pub(crate) fn runtime_root() -> Option<&'static Path> {
    static ROOT: OnceLock<Option<PathBuf>> = OnceLock::new();
    ROOT.get_or_init(|| {
        if MODULE.load(Ordering::Relaxed).is_null() {
            executable_root().ok()
        } else {
            dll_root().ok()
        }
    })
    .as_deref()
}

pub(crate) fn installed(root: &Path) -> bool {
    root.join("bin").is_dir()
}

pub(crate) fn binaries(root: &Path) -> PathBuf {
    root.join(if installed(root) { "bin" } else { "outputs" })
}

pub(crate) fn maintenance(root: &Path) -> PathBuf {
    let release = crate::diagnostics::RELEASE.trim();
    binaries(root).join(format!("oxide_ime_maintenance_{release}.exe"))
}

const DATA_KEY: &str = r"Software\OxideIME";
const LOCAL_APP_DATA: Guid = Guid::new(
    0xf1b32785,
    0x6fba,
    0x4fcf,
    [0x9d, 0x55, 0x7b, 0x8e, 0x7f, 0x15, 0x70, 0x91],
);

#[link(name = "shell32")]
extern "system" {
    fn SHGetKnownFolderPath(
        folder: *const Guid,
        flags: u32,
        token: Raw,
        path: *mut *mut u16,
    ) -> HResult;
}

#[link(name = "ole32")]
extern "system" {
    fn CoTaskMemFree(memory: Raw);
}

pub(crate) fn user_root() -> Result<PathBuf, String> {
    unsafe {
        let mut path = std::ptr::null_mut();
        let hr = SHGetKnownFolderPath(&LOCAL_APP_DATA, 0, std::ptr::null_mut(), &mut path);
        if hr < 0 || path.is_null() {
            return Err(format!("找不到使用者資料目錄（0x{:08X}）", hr as u32));
        }
        let len = (0..).take_while(|index| *path.add(*index) != 0).count();
        let directory = PathBuf::from(OsString::from_wide(std::slice::from_raw_parts(path, len)));
        CoTaskMemFree(path.cast());
        Ok(directory.join("OxideIME"))
    }
}

pub(crate) fn user_data(root: &Path) -> Result<PathBuf, String> {
    if !installed(root) {
        return Ok(root.join("data"));
    }
    if let Some(directory) = configured_data_directory() {
        return Ok(directory);
    }
    Ok(user_root()?.join("data"))
}

fn configured_data_directory() -> Option<PathBuf> {
    let key = wide(DATA_KEY);
    let name = wide("DataDirectory");
    let mut path = vec![0u16; 32768];
    let mut size = (path.len() * 2) as u32;
    let result = unsafe {
        registry::RegGetValueW(
            registry::HKCU,
            key.as_ptr(),
            name.as_ptr(),
            2, // RRF_RT_REG_SZ
            std::ptr::null_mut(),
            path.as_mut_ptr().cast(),
            &mut size,
        )
    };
    if result != 0 {
        return None;
    }
    let directory = PathBuf::from(std::ffi::OsString::from_wide(
        path.split(|unit| *unit == 0).next()?,
    ));
    directory.is_absolute().then_some(directory)
}

pub(crate) fn configure_user_data(directory: &Path) -> Result<(), String> {
    let (key, _) = unsafe { registry::Key::create(registry::HKCU, DATA_KEY, 0, 2) }
        .map_err(|rc| format!("無法開啟資料目錄設定（{rc}）"))?;
    key.set_string(Some("DataDirectory"), directory.as_os_str())
        .map_err(|rc| format!("無法寫入登錄設定（{rc}）"))
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct FileStamp {
    pub size: u64,
    modified: SystemTime,
}

impl FileStamp {
    pub(crate) fn read(path: &Path) -> std::io::Result<Self> {
        let metadata = std::fs::metadata(path)?;
        Ok(Self {
            size: metadata.len(),
            modified: metadata.modified()?,
        })
    }
}

/// 從同一個目錄發佈檔案，避免讀取者看見未寫完的內容。
pub(crate) fn write_staged(
    destination: &Path,
    contents: &[u8],
    publish: impl FnOnce(&Path, &Path) -> io::Result<()>,
) -> io::Result<()> {
    static NEXT_STAGE: AtomicU32 = AtomicU32::new(0);
    let (stage, mut file) = loop {
        let sequence = NEXT_STAGE.fetch_add(1, Ordering::Relaxed);
        let path =
            destination.with_file_name(format!(".oxide-{}-{sequence}.tmp", std::process::id()));
        match fs::File::create_new(&path) {
            Ok(file) => break (path, file),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    };
    let written = file.write_all(contents).and_then(|()| file.sync_all());
    drop(file);
    let result = written.and_then(|()| publish(&stage, destination));
    if result.is_err() {
        let _ = fs::remove_file(stage);
    }
    result
}

pub(crate) fn replace_file(destination: &Path, contents: &[u8]) -> io::Result<()> {
    write_staged(destination, contents, |stage, destination| {
        fs::rename(stage, destination)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn staged_writes_do_not_share_temp_files_or_leave_failed_payloads() {
        let root = crate::tests::fixture_dir("staged-write");
        let destination = root.join("existing");
        fs::write(&destination, b"original").unwrap();
        assert!(write_staged(&destination, b"other", |stage, _| {
            assert_eq!(fs::read(stage).unwrap(), b"other");
            Err(io::Error::from(io::ErrorKind::PermissionDenied))
        })
        .is_err());
        assert_eq!(fs::read(&destination).unwrap(), b"original");
        assert_eq!(fs::read_dir(&root).unwrap().count(), 1);

        let ready = std::sync::Barrier::new(4);
        std::thread::scope(|scope| {
            for id in 0..4u8 {
                let name = root.join(id.to_string());
                let ready = &ready;
                scope.spawn(move || {
                    ready.wait();
                    replace_file(&name, &[id]).unwrap();
                    assert_eq!(fs::read(name).unwrap(), [id]);
                });
            }
        });
        assert_eq!(fs::read_dir(&root).unwrap().count(), 5);
        crate::tests::remove_fixture(&root);
    }
}
