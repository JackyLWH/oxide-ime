//! Windows TSF 的 COM 介面、文字服務和原生顯示。
mod candidate_window;
mod com;
mod composition;
mod display_attribute;
pub(super) mod exports;
pub(crate) mod ffi;
mod font;
mod gdi;
mod language_bar;
mod maintenance_menu;
pub(crate) mod registration;
pub(crate) mod service;
mod ui_element;

use std::sync::atomic::{AtomicIsize, AtomicPtr};

use ffi::Guid;

pub(crate) static MODULE: AtomicPtr<std::ffi::c_void> = AtomicPtr::new(std::ptr::null_mut());
pub(crate) static OBJECTS: AtomicIsize = AtomicIsize::new(0);

pub(crate) const CLSID: Guid = Guid::new(
    0x4bd0700e,
    0x5dd1,
    0x4310,
    [0xa9, 0xd2, 0xf3, 0xd5, 0x79, 0xd5, 0x47, 0x30],
);
pub(crate) const PROFILE: Guid = Guid::new(
    0x21a75529,
    0x20a1,
    0x4cf2,
    [0xb7, 0xd8, 0x1b, 0xb9, 0x42, 0x67, 0xe1, 0xc9],
);
pub(crate) const LANGUAGE: u16 = 0x0404; // 中文（繁體，台灣）
