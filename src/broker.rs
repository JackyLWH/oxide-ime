//! 為受 AppContainer 限制的宿主提供同一登入工作階段的字庫服務。
mod client;
mod modes;
mod permissions;
mod pipe;
mod protocol;
mod server;

pub(crate) use client::{
    check, clipboard_enabled, convert_clipboard, homophones, jyutping, lookup, modes, resolve,
    resolve_original, reverse, reverse_match,
};
pub(crate) use modes::{ModeAction, Modes};
pub(crate) use pipe::{current_logon_sid, is_appcontainer, is_elevated};
pub(crate) use server::serve;
#[cfg(feature = "integration-fixtures")]
pub(crate) use server::set_test_permission;
