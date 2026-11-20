//! Oxide IME 的 Windows 文字服務及共用輸入引擎。

#[cfg(not(all(windows, any(target_arch = "x86_64", target_arch = "x86"))))]
compile_error!("此原型只支援 x64 或 x86 Windows。");

mod broker;
mod commands;
mod conversion;
mod diagnostics;
mod dictionary;
mod engine;
mod homophone;
mod maintenance;
mod project_files;
mod public_data;
mod punctuation;
mod simplify;
mod tsf;
mod update;
mod win32;

#[cfg(feature = "integration-fixtures")]
pub use tsf::exports::RustImeSyntheticClipboardFixture;
pub use tsf::exports::{
    DllCanUnloadNow, DllGetClassObject, DllInstall, DllMain, DllRegisterServer, DllUnregisterServer,
};

pub(crate) const PRODUCT_NAME: &str = "Oxide IME";

#[doc(hidden)]
pub fn run_maintenance() -> Result<(), String> {
    maintenance::run()
}

#[cfg(test)]
mod tests;

#[doc(hidden)]
pub fn run_dictionary_broker(replace: bool) -> Result<(), String> {
    let result = broker::serve(replace);
    if result.is_err() {
        diagnostics::event(diagnostics::Level::Error, "broker", "start_failed", &[]);
    }
    result
}

#[doc(hidden)]
pub fn check_dictionary_broker(expected_pid: Option<u32>) -> Result<(), String> {
    broker::check(expected_pid)
}

#[doc(hidden)]
#[cfg(feature = "integration-fixtures")]
pub fn set_test_sandbox_permission(sid: &str, enabled: bool) -> Result<(), String> {
    broker::set_test_permission(sid, enabled)
}
