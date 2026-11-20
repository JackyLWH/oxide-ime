//! 僅宣告原型所需的 Windows SDK COM ABI；介面順序依 msctf.h。

use std::ffi::c_void;
use std::ptr;

pub type HResult = i32;
pub type Raw = *mut c_void;
pub const S_OK: HResult = 0;
pub const S_FALSE: HResult = 1;
pub const E_FAIL: HResult = 0x80004005u32 as i32;
pub const E_NOTIMPL: HResult = 0x80004001u32 as i32;
pub const E_NOINTERFACE: HResult = 0x80004002u32 as i32;
pub const E_POINTER: HResult = 0x80004003u32 as i32;
pub const E_UNEXPECTED: HResult = 0x8000ffffu32 as i32;
pub const E_INVALIDARG: HResult = 0x80070057u32 as i32;
pub const E_OUTOFMEMORY: HResult = 0x8007000eu32 as i32;
pub const CLASS_E_NOAGGREGATION: HResult = 0x80040110u32 as i32;
pub const CLASS_E_CLASSNOTAVAILABLE: HResult = 0x80040111u32 as i32;
pub const RPC_E_CHANGED_MODE: HResult = 0x80010106u32 as i32;
pub const TF_ES_SYNC: u32 = 0x1;
pub const TF_ES_READ: u32 = 0x2;
pub const TF_ES_READWRITE: u32 = 0x6;
pub const TF_ES_ASYNC: u32 = 0x8;
pub const TF_E_SYNCHRONOUS: HResult = 0x80040208u32 as i32;
pub const TF_S_ASYNC: HResult = 0x00040300;
pub const TF_TMAE_UIELEMENTENABLEDONLY: u32 = 0x4;
pub const TF_IAS_NO_DEFAULT_COMPOSITION: u32 = 0x80000000;

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Guid {
    pub data1: u32,
    pub data2: u16,
    pub data3: u16,
    pub data4: [u8; 8],
}

impl Guid {
    pub const fn new(data1: u32, data2: u16, data3: u16, data4: [u8; 8]) -> Self {
        Self {
            data1,
            data2,
            data3,
            data4,
        }
    }
}

#[repr(C)]
pub struct UnknownVtbl {
    pub query: unsafe extern "system" fn(Raw, *const Guid, *mut Raw) -> HResult,
    pub add_ref: unsafe extern "system" fn(Raw) -> u32,
    pub release: unsafe extern "system" fn(Raw) -> u32,
}

#[repr(C)]
pub struct ClassFactoryVtbl {
    pub base: UnknownVtbl,
    pub create_instance: unsafe extern "system" fn(Raw, Raw, *const Guid, *mut Raw) -> HResult,
    pub lock_server: unsafe extern "system" fn(Raw, i32) -> HResult,
}

#[cfg(test)]
#[repr(C)]
pub struct TextInputProcessorVtbl {
    pub base: UnknownVtbl,
    pub activate: unsafe extern "system" fn(Raw, Raw, u32) -> HResult,
    pub deactivate: unsafe extern "system" fn(Raw) -> HResult,
}

#[repr(C)]
pub struct TextInputProcessorExVtbl {
    pub base: UnknownVtbl,
    pub activate: unsafe extern "system" fn(Raw, Raw, u32) -> HResult,
    pub deactivate: unsafe extern "system" fn(Raw) -> HResult,
    pub activate_ex: unsafe extern "system" fn(Raw, Raw, u32, u32) -> HResult,
}

#[repr(C)]
pub struct UiElementMgrVtbl {
    pub base: UnknownVtbl,
    pub begin: unsafe extern "system" fn(Raw, Raw, *mut i32, *mut u32) -> HResult,
    pub update: unsafe extern "system" fn(Raw, u32) -> HResult,
    pub end: unsafe extern "system" fn(Raw, u32) -> HResult,
}

#[repr(C)]
pub struct UiElementVtbl {
    pub base: UnknownVtbl,
    pub description: unsafe extern "system" fn(Raw, *mut Raw) -> HResult,
    pub guid: unsafe extern "system" fn(Raw, *mut Guid) -> HResult,
    pub show: unsafe extern "system" fn(Raw, i32) -> HResult,
    pub is_shown: unsafe extern "system" fn(Raw, *mut i32) -> HResult,
}

#[repr(C)]
pub struct ReadingUiVtbl {
    pub base: UiElementVtbl,
    pub flags: unsafe extern "system" fn(Raw, *mut u32) -> HResult,
    pub context: unsafe extern "system" fn(Raw, *mut Raw) -> HResult,
    pub string: unsafe extern "system" fn(Raw, *mut Raw) -> HResult,
    pub max_length: unsafe extern "system" fn(Raw, *mut u32) -> HResult,
    pub error_index: unsafe extern "system" fn(Raw, *mut u32) -> HResult,
    pub vertical: unsafe extern "system" fn(Raw, *mut i32) -> HResult,
}

#[repr(C)]
pub struct CandidateUiVtbl {
    pub base: UnknownVtbl,
    pub description: unsafe extern "system" fn(Raw, *mut Raw) -> HResult,
    pub guid: unsafe extern "system" fn(Raw, *mut Guid) -> HResult,
    pub show: unsafe extern "system" fn(Raw, i32) -> HResult,
    pub is_shown: unsafe extern "system" fn(Raw, *mut i32) -> HResult,
    pub flags: unsafe extern "system" fn(Raw, *mut u32) -> HResult,
    pub document: unsafe extern "system" fn(Raw, *mut Raw) -> HResult,
    pub count: unsafe extern "system" fn(Raw, *mut u32) -> HResult,
    pub selection: unsafe extern "system" fn(Raw, *mut u32) -> HResult,
    pub string: unsafe extern "system" fn(Raw, u32, *mut Raw) -> HResult,
    pub page_index: unsafe extern "system" fn(Raw, *mut u32, u32, *mut u32) -> HResult,
    pub set_page_index: unsafe extern "system" fn(Raw, *const u32, u32) -> HResult,
    pub current_page: unsafe extern "system" fn(Raw, *mut u32) -> HResult,
    pub set_selection: unsafe extern "system" fn(Raw, u32) -> HResult,
    pub finalize: unsafe extern "system" fn(Raw) -> HResult,
    pub abort: unsafe extern "system" fn(Raw) -> HResult,
}

#[repr(C)]
pub struct KeyEventSinkVtbl {
    pub base: UnknownVtbl,
    pub on_set_focus: unsafe extern "system" fn(Raw, i32) -> HResult,
    pub on_test_key_down: unsafe extern "system" fn(Raw, Raw, usize, isize, *mut i32) -> HResult,
    pub on_test_key_up: unsafe extern "system" fn(Raw, Raw, usize, isize, *mut i32) -> HResult,
    pub on_key_down: unsafe extern "system" fn(Raw, Raw, usize, isize, *mut i32) -> HResult,
    pub on_key_up: unsafe extern "system" fn(Raw, Raw, usize, isize, *mut i32) -> HResult,
    pub on_preserved_key: unsafe extern "system" fn(Raw, Raw, *const Guid, *mut i32) -> HResult,
}

#[repr(C)]
pub struct KeyStrokeMgrVtbl {
    pub base: UnknownVtbl,
    pub advise: unsafe extern "system" fn(Raw, u32, Raw, i32) -> HResult,
    pub unadvise: unsafe extern "system" fn(Raw, u32) -> HResult,
}

#[repr(C)]
pub struct ThreadManagerVtbl {
    pub base: UnknownVtbl,
    pub activate: unsafe extern "system" fn(Raw, *mut u32) -> HResult,
    pub deactivate: unsafe extern "system" fn(Raw) -> HResult,
    pub before_focus: [usize; 2],
    pub get_focus: unsafe extern "system" fn(Raw, *mut Raw) -> HResult,
    pub before_global_compartment: [usize; 5],
    pub get_global_compartment: unsafe extern "system" fn(Raw, *mut Raw) -> HResult,
}

#[repr(C)]
pub struct CompartmentMgrVtbl {
    pub base: UnknownVtbl,
    pub get_compartment: unsafe extern "system" fn(Raw, *const Guid, *mut Raw) -> HResult,
}

#[repr(C)]
pub struct CompartmentVtbl {
    pub base: UnknownVtbl,
    pub set_value: unsafe extern "system" fn(Raw, u32, *const Variant) -> HResult,
}

#[repr(C)]
pub struct CompartmentSinkVtbl {
    pub base: UnknownVtbl,
    pub on_change: unsafe extern "system" fn(Raw, *const Guid) -> HResult,
}

#[repr(C)]
pub struct ContextVtbl {
    pub base: UnknownVtbl,
    pub request_edit_session:
        unsafe extern "system" fn(Raw, u32, Raw, u32, *mut HResult) -> HResult,
}

#[repr(C)]
pub struct Selection {
    pub range: Raw,
    pub active_end: i32,
    pub interim: i32,
}

#[repr(C)]
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub struct Rect {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

#[repr(C)]
pub struct ContextFullVtbl {
    pub base: UnknownVtbl,
    pub request_edit_session:
        unsafe extern "system" fn(Raw, u32, Raw, u32, *mut HResult) -> HResult,
    pub in_write_session: usize,
    pub get_selection:
        unsafe extern "system" fn(Raw, u32, u32, u32, *mut Selection, *mut u32) -> HResult,
    pub set_selection: unsafe extern "system" fn(Raw, u32, u32, *const Selection) -> HResult,
    pub get_start: usize,
    pub get_end: usize,
    pub get_active_view: unsafe extern "system" fn(Raw, *mut Raw) -> HResult,
    pub before_property: [usize; 2],
    pub get_property: unsafe extern "system" fn(Raw, *const Guid, *mut Raw) -> HResult,
    pub before_document_mgr: [usize; 3],
    pub get_document_mgr: unsafe extern "system" fn(Raw, *mut Raw) -> HResult,
}

#[repr(C)]
pub union VariantData {
    pub long: i32,
    pub storage: [usize; 2],
}

#[repr(C)]
pub struct Variant {
    pub kind: u16,
    pub reserved: [u16; 3],
    pub data: VariantData,
}

#[repr(C)]
pub struct PropertyVtbl {
    pub base: UnknownVtbl,
    pub get_type: usize,
    pub enum_ranges: usize,
    pub get_value: unsafe extern "system" fn(Raw, u32, Raw, *mut Variant) -> HResult,
    pub get_context: usize,
    pub find_range: usize,
    pub set_value_store: usize,
    pub set_value: unsafe extern "system" fn(Raw, u32, Raw, *const Variant) -> HResult,
    pub clear: unsafe extern "system" fn(Raw, u32, Raw) -> HResult,
}

#[repr(C)]
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub struct DisplayColor {
    pub kind: i32,
    pub value: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub struct DisplayAttribute {
    pub text: DisplayColor,
    pub background: DisplayColor,
    pub line_style: i32,
    pub bold_line: i32,
    pub line_color: DisplayColor,
    pub attribute: i32,
}

#[repr(C)]
pub struct DisplayAttributeProviderVtbl {
    pub base: UnknownVtbl,
    pub enumerate: unsafe extern "system" fn(Raw, *mut Raw) -> HResult,
    pub get_info: unsafe extern "system" fn(Raw, *const Guid, *mut Raw) -> HResult,
}

#[repr(C)]
pub struct DisplayAttributeInfoVtbl {
    pub base: UnknownVtbl,
    pub get_guid: unsafe extern "system" fn(Raw, *mut Guid) -> HResult,
    pub get_description: unsafe extern "system" fn(Raw, *mut Raw) -> HResult,
    pub get_attribute: unsafe extern "system" fn(Raw, *mut DisplayAttribute) -> HResult,
    pub set_attribute: unsafe extern "system" fn(Raw, *const DisplayAttribute) -> HResult,
    pub reset: unsafe extern "system" fn(Raw) -> HResult,
}

#[repr(C)]
pub struct EnumDisplayAttributeInfoVtbl {
    pub base: UnknownVtbl,
    pub clone_enum: unsafe extern "system" fn(Raw, *mut Raw) -> HResult,
    pub next: unsafe extern "system" fn(Raw, u32, *mut Raw, *mut u32) -> HResult,
    pub reset: unsafe extern "system" fn(Raw) -> HResult,
    pub skip: unsafe extern "system" fn(Raw, u32) -> HResult,
}

#[repr(C)]
pub struct ContextViewVtbl {
    pub base: UnknownVtbl,
    pub get_range_from_point: usize,
    pub get_text_ext: unsafe extern "system" fn(Raw, u32, Raw, *mut Rect, *mut i32) -> HResult,
    pub get_screen_ext: usize,
    pub get_window: unsafe extern "system" fn(Raw, *mut Raw) -> HResult,
}

#[repr(C)]
pub struct ContextCompositionVtbl {
    pub base: UnknownVtbl,
    pub start: unsafe extern "system" fn(Raw, u32, Raw, Raw, *mut Raw) -> HResult,
}

#[repr(C)]
pub struct CompositionVtbl {
    pub base: UnknownVtbl,
    pub get_range: unsafe extern "system" fn(Raw, *mut Raw) -> HResult,
    pub shift_start: usize,
    pub shift_end: usize,
    pub end: unsafe extern "system" fn(Raw, u32) -> HResult,
}

#[repr(C)]
pub struct CompositionSinkVtbl {
    pub base: UnknownVtbl,
    pub terminated: unsafe extern "system" fn(Raw, u32, Raw) -> HResult,
}

#[repr(C)]
pub struct RangeVtbl {
    pub base: UnknownVtbl,
    pub get_text: unsafe extern "system" fn(Raw, u32, u32, *mut u16, u32, *mut u32) -> HResult,
    pub set_text: unsafe extern "system" fn(Raw, u32, u32, *const u16, i32) -> HResult,
    pub before_shift_end: [usize; 4],
    pub shift_end: unsafe extern "system" fn(Raw, u32, i32, *mut i32, Raw) -> HResult,
    pub before_is_empty: [usize; 4],
    pub is_empty: unsafe extern "system" fn(Raw, u32, *mut i32) -> HResult,
    pub collapse: unsafe extern "system" fn(Raw, u32, i32) -> HResult,
    pub before_compare_start: [usize; 2],
    pub compare_start: unsafe extern "system" fn(Raw, u32, Raw, i32, *mut i32) -> HResult,
    pub before_clone: [usize; 4],
    pub clone_range: unsafe extern "system" fn(Raw, *mut Raw) -> HResult,
}

#[repr(C)]
pub struct EditSessionVtbl {
    pub base: UnknownVtbl,
    pub do_edit_session: unsafe extern "system" fn(Raw, u32) -> HResult,
}

#[repr(C)]
pub struct InsertAtSelectionVtbl {
    pub base: UnknownVtbl,
    pub insert_text: unsafe extern "system" fn(Raw, u32, u32, *const u16, i32, *mut Raw) -> HResult,
}

#[repr(C)]
pub struct LangBarInfo {
    pub service: Guid,
    pub item: Guid,
    pub style: u32,
    pub sort: u32,
    pub description: [u16; 32],
}

#[repr(C)]
pub struct LangBarMgrVtbl {
    pub base: UnknownVtbl,
    pub enum_items: usize,
    pub get_item: usize,
    pub add_item: unsafe extern "system" fn(Raw, Raw) -> HResult,
    pub remove_item: unsafe extern "system" fn(Raw, Raw) -> HResult,
}

#[repr(C)]
pub struct LangBarButtonVtbl {
    pub base: UnknownVtbl,
    pub get_info: unsafe extern "system" fn(Raw, *mut LangBarInfo) -> HResult,
    pub get_status: unsafe extern "system" fn(Raw, *mut u32) -> HResult,
    pub show: unsafe extern "system" fn(Raw, i32) -> HResult,
    pub get_tooltip: unsafe extern "system" fn(Raw, *mut Raw) -> HResult,
    pub on_click: unsafe extern "system" fn(Raw, i32, i64, *const Rect) -> HResult,
    pub init_menu: unsafe extern "system" fn(Raw, Raw) -> HResult,
    pub menu_select: unsafe extern "system" fn(Raw, u32) -> HResult,
    pub get_icon: unsafe extern "system" fn(Raw, *mut Raw) -> HResult,
    pub get_text: unsafe extern "system" fn(Raw, *mut Raw) -> HResult,
}

#[repr(C)]
pub struct SourceVtbl {
    pub base: UnknownVtbl,
    pub advise: unsafe extern "system" fn(Raw, *const Guid, Raw, *mut u32) -> HResult,
    pub unadvise: unsafe extern "system" fn(Raw, u32) -> HResult,
}

#[repr(C)]
pub struct LangBarSinkVtbl {
    pub base: UnknownVtbl,
    pub on_update: unsafe extern "system" fn(Raw, u32) -> HResult,
}

#[repr(C)]
pub struct ProfileMgrVtbl {
    pub base: UnknownVtbl,
    pub activate_profile:
        unsafe extern "system" fn(Raw, u32, u16, *const Guid, *const Guid, Raw, u32) -> HResult,
    pub deactivate_profile:
        unsafe extern "system" fn(Raw, u32, u16, *const Guid, *const Guid, Raw, u32) -> HResult,
    pub get_profile: usize,
    pub enum_profiles: usize,
    pub release_processor: unsafe extern "system" fn(Raw, *const Guid, u32) -> HResult,
    pub register_profile: unsafe extern "system" fn(
        Raw,
        *const Guid,
        u16,
        *const Guid,
        *const u16,
        u32,
        *const u16,
        u32,
        u32,
        Raw,
        u32,
        i32,
        u32,
    ) -> HResult,
    pub unregister_profile:
        unsafe extern "system" fn(Raw, *const Guid, u16, *const Guid, u32) -> HResult,
    pub get_active_profile:
        unsafe extern "system" fn(Raw, *const Guid, *mut InputProcessorProfile) -> HResult,
}

#[repr(C)]
pub struct InputProcessorProfile {
    pub kind: u32,
    pub langid: u16,
    pub clsid: Guid,
    pub profile: Guid,
    pub category: Guid,
    pub substitute: Raw,
    pub capabilities: u32,
    pub layout: Raw,
    pub flags: u32,
}

#[repr(C)]
pub struct CategoryMgrVtbl {
    pub base: UnknownVtbl,
    pub register_category:
        unsafe extern "system" fn(Raw, *const Guid, *const Guid, *const Guid) -> HResult,
    pub unregister_category:
        unsafe extern "system" fn(Raw, *const Guid, *const Guid, *const Guid) -> HResult,
    pub before_register_guid: [usize; 9],
    pub register_guid: unsafe extern "system" fn(Raw, *const Guid, *mut u32) -> HResult,
}

#[link(name = "uuid", kind = "static", modifiers = "-bundle")]
extern "system" {
    pub static IID_IUnknown: Guid;
    pub static IID_IClassFactory: Guid;
    pub static IID_ITfTextInputProcessor: Guid;
    pub static IID_ITfTextInputProcessorEx: Guid;
    pub static IID_ITfKeyEventSink: Guid;
    pub static IID_ITfKeystrokeMgr: Guid;
    pub static IID_ITfEditSession: Guid;
    pub static IID_ITfInsertAtSelection: Guid;
    pub static IID_ITfContextComposition: Guid;
    pub static IID_ITfCompositionSink: Guid;
    pub static IID_ITfInputProcessorProfileMgr: Guid;
    pub static IID_ITfCategoryMgr: Guid;
    pub static CLSID_TF_InputProcessorProfiles: Guid;
    pub static CLSID_TF_ThreadMgr: Guid;
    pub static IID_ITfThreadMgr: Guid;
    pub static CLSID_TF_CategoryMgr: Guid;
    pub static GUID_TFCAT_TIP_KEYBOARD: Guid;
    pub static GUID_TFCAT_TIPCAP_SYSTRAYSUPPORT: Guid;
    pub static GUID_TFCAT_TIPCAP_UIELEMENTENABLED: Guid;
    pub static GUID_TFCAT_TIPCAP_IMMERSIVESUPPORT: Guid;
    pub static GUID_TFCAT_DISPLAYATTRIBUTEPROVIDER: Guid;
    pub static GUID_PROP_ATTRIBUTE: Guid;
    pub static IID_ITfDisplayAttributeProvider: Guid;
    pub static IID_ITfDisplayAttributeInfo: Guid;
    pub static IID_IEnumTfDisplayAttributeInfo: Guid;
    pub static IID_ITfUIElementMgr: Guid;
    pub static IID_ITfUIElement: Guid;
    pub static IID_ITfReadingInformationUIElement: Guid;
    pub static IID_ITfCandidateListUIElement: Guid;
    pub static IID_ITfCandidateListUIElementBehavior: Guid;
    pub static IID_ITfLangBarItemMgr: Guid;
    pub static IID_ITfLangBarItem: Guid;
    pub static IID_ITfLangBarItemButton: Guid;
    pub static IID_ITfLangBarItemSink: Guid;
    pub static IID_ITfSource: Guid;
    pub static IID_ITfCompartmentEventSink: Guid;
    pub static GUID_LBI_INPUTMODE: Guid;
}

#[link(name = "ole32")]
extern "system" {
    pub fn CoInitializeEx(reserved: Raw, coinit: u32) -> HResult;
    pub fn CoUninitialize();
    pub fn CoCreateInstance(
        clsid: *const Guid,
        outer: Raw,
        context: u32,
        iid: *const Guid,
        out: *mut Raw,
    ) -> HResult;
}

#[link(name = "kernel32")]
extern "system" {
    pub fn GetModuleFileNameW(module: Raw, filename: *mut u16, size: u32) -> u32;
}

#[link(name = "user32")]
extern "system" {
    pub fn GetKeyState(key: i32) -> i16;
}

pub unsafe fn vtable<T>(object: Raw) -> &'static T {
    &**(object as *const *const T)
}

pub unsafe fn query(object: Raw, iid: *const Guid) -> Result<Raw, HResult> {
    let mut result = ptr::null_mut();
    let hr = (vtable::<UnknownVtbl>(object).query)(object, iid, &mut result);
    checked_out(hr, result)
}

fn checked_out(hr: HResult, object: Raw) -> Result<Raw, HResult> {
    if hr < 0 {
        Err(hr)
    } else if object.is_null() {
        Err(E_UNEXPECTED)
    } else {
        Ok(object)
    }
}

pub unsafe fn add_ref(object: Raw) -> u32 {
    (vtable::<UnknownVtbl>(object).add_ref)(object)
}

pub unsafe fn release(object: Raw) -> u32 {
    (vtable::<UnknownVtbl>(object).release)(object)
}

pub struct ComPtr(Raw);

impl ComPtr {
    pub unsafe fn from_out(hr: HResult, object: Raw) -> Result<Self, HResult> {
        checked_out(hr, object).map(Self)
    }

    pub unsafe fn retain(object: Raw) -> Self {
        add_ref(object);
        Self(object)
    }

    pub unsafe fn query(object: Raw, iid: *const Guid) -> Result<Self, HResult> {
        query(object, iid).map(Self)
    }

    pub fn as_raw(&self) -> Raw {
        self.0
    }
}

impl Drop for ComPtr {
    fn drop(&mut self) {
        unsafe {
            release(self.0);
        }
    }
}

pub unsafe fn create_instance(clsid: *const Guid, iid: *const Guid) -> Result<ComPtr, HResult> {
    let mut result = ptr::null_mut();
    let hr = CoCreateInstance(clsid, ptr::null_mut(), 1, iid, &mut result);
    ComPtr::from_out(hr, result)
}

#[link(name = "oleaut32")]
extern "system" {
    fn SysAllocStringLen(text: *const u16, length: u32) -> Raw;
}

pub(super) unsafe fn write_bstr(value: &str, out: *mut Raw) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    let text: Vec<u16> = value.encode_utf16().collect();
    *out = SysAllocStringLen(text.as_ptr(), text.len() as u32);
    if (*out).is_null() {
        E_OUTOFMEMORY
    } else {
        S_OK
    }
}
