//! 輸入服務及字庫服務共用的 Windows API。
pub(crate) mod registry;

use std::ffi::{c_void, OsStr};
use std::os::windows::ffi::OsStrExt;
use std::os::windows::process::CommandExt;
use std::process::Command;

#[repr(C)]
#[derive(Default)]
pub(crate) struct LocalTime {
    pub(crate) year: u16,
    pub(crate) month: u16,
    day_of_week: u16,
    pub(crate) day: u16,
    pub(crate) hour: u16,
    pub(crate) minute: u16,
    pub(crate) second: u16,
    milliseconds: u16,
}

impl LocalTime {
    pub(crate) fn now() -> Self {
        let mut time = Self::default();
        unsafe { GetLocalTime(&mut time) };
        time
    }
}

pub(crate) fn wide(text: &str) -> Vec<u16> {
    wide_os(OsStr::new(text))
}

pub(crate) fn wide_os(text: &OsStr) -> Vec<u16> {
    text.encode_wide().chain([0]).collect()
}

pub(crate) fn allow_foreground(process: u32) {
    unsafe { AllowSetForegroundWindow(process) };
}

pub(crate) fn hidden_command(program: impl AsRef<OsStr>) -> Command {
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let mut command = Command::new(program);
    command.creation_flags(CREATE_NO_WINDOW);
    command
}

#[link(name = "kernel32")]
extern "system" {
    fn GetLocalTime(time: *mut LocalTime);
    pub(crate) fn GetCurrentThreadId() -> u32;
    pub(crate) fn GetLastError() -> u32;
}

#[link(name = "user32")]
extern "system" {
    fn AllowSetForegroundWindow(process: u32) -> i32;
    pub(crate) fn GetFocus() -> *mut c_void;
    pub(crate) fn GetForegroundWindow() -> *mut c_void;
    pub(crate) fn GetAncestor(window: *mut c_void, flags: u32) -> *mut c_void;
}
