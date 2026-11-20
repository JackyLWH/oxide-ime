//! 不修改系統設定：模擬文字服務宿主，並驗證真正的 Windows TSF 管理員可啟用。
use std::mem::offset_of;
use std::ptr;
use std::sync::atomic::{AtomicU32, Ordering};

use crate::tsf::ffi::{
    self, ClassFactoryVtbl, ContextVtbl, EditSessionVtbl, Guid, HResult, InsertAtSelectionVtbl,
    KeyEventSinkVtbl, KeyStrokeMgrVtbl, Raw, TextInputProcessorVtbl, UnknownVtbl,
};
use crate::tsf::CLSID;
use crate::{DllCanUnloadNow, DllGetClassObject};

#[link(name = "user32")]
extern "system" {
    fn GetKeyboardState(state: *mut u8) -> i32;
    fn SetKeyboardState(state: *const u8) -> i32;
}

struct TestKeyboardState([u8; 256]);

impl TestKeyboardState {
    fn neutral() -> Self {
        let mut saved = Self([0; 256]);
        unsafe {
            assert_ne!(GetKeyboardState(saved.0.as_mut_ptr()), 0);
            assert_ne!(SetKeyboardState([0u8; 256].as_ptr()), 0);
        }
        saved
    }
}

impl Drop for TestKeyboardState {
    fn drop(&mut self) {
        // 只還原目前執行測試的執行緒，不送出實際按鍵。
        unsafe { SetKeyboardState(self.0.as_ptr()) };
    }
}

/// 測試只建立專案 work 內的唯一目錄，避免覆寫先前的診斷資料。
pub(crate) fn fixture_dir(prefix: &str) -> std::path::PathBuf {
    assert!(prefix
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || ch == '-'));
    let project = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .canonicalize()
        .unwrap();
    let work = project.join("work");
    std::fs::create_dir_all(&work).unwrap();
    let work = work.canonicalize().unwrap();
    assert!(work.starts_with(&project));
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = work.join(format!("{prefix}-{}-{nonce}", std::process::id()));
    std::fs::create_dir(&dir).unwrap();
    dir
}

/// 只清理此程序在 work 內建立的隔離測試目錄。
pub(crate) fn remove_fixture(dir: &std::path::Path) {
    let work = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("work")
        .canonicalize()
        .unwrap();
    let dir = dir.canonicalize().unwrap();
    assert_eq!(dir.parent(), Some(work.as_path()));
    let name = dir.file_name().and_then(|name| name.to_str()).unwrap();
    let (prefix_and_pid, nonce) = name.rsplit_once('-').unwrap();
    let (_, pid) = prefix_and_pid.rsplit_once('-').unwrap();
    assert_eq!(pid.parse::<u32>().unwrap(), std::process::id());
    assert!(nonce.parse::<u128>().is_ok());
    std::fs::remove_dir_all(dir).unwrap();
}

#[repr(C)]
struct FakeManager {
    vtbl: *const KeyStrokeMgrVtbl,
    refs: AtomicU32,
    sink: Raw,
    id: u32,
}

static MANAGER_VTBL: KeyStrokeMgrVtbl = KeyStrokeMgrVtbl {
    base: UnknownVtbl {
        query: manager_query,
        add_ref: manager_add_ref,
        release: manager_release,
    },
    advise: manager_advise,
    unadvise: manager_unadvise,
};

unsafe extern "system" fn manager_query(this: Raw, iid: *const Guid, out: *mut Raw) -> HResult {
    *out = ptr::null_mut();
    if *iid != ffi::IID_IUnknown && *iid != ffi::IID_ITfKeystrokeMgr {
        return ffi::E_NOINTERFACE;
    }
    *out = this;
    manager_add_ref(this);
    ffi::S_OK
}
unsafe extern "system" fn manager_add_ref(this: Raw) -> u32 {
    (*(this as *mut FakeManager))
        .refs
        .fetch_add(1, Ordering::SeqCst)
        + 1
}
unsafe extern "system" fn manager_release(this: Raw) -> u32 {
    (*(this as *mut FakeManager))
        .refs
        .fetch_sub(1, Ordering::SeqCst)
        - 1
}
unsafe extern "system" fn manager_advise(
    this: Raw,
    id: u32,
    sink: Raw,
    foreground: i32,
) -> HResult {
    let manager = &mut *(this as *mut FakeManager);
    assert_eq!(foreground, 1);
    assert!(manager.sink.is_null());
    manager.id = id;
    manager.sink = sink;
    ffi::add_ref(sink);
    ffi::S_OK
}
unsafe extern "system" fn manager_unadvise(this: Raw, id: u32) -> HResult {
    let manager = &mut *(this as *mut FakeManager);
    assert_eq!(manager.id, id);
    assert!(!manager.sink.is_null());
    ffi::release(manager.sink);
    manager.sink = ptr::null_mut();
    ffi::S_OK
}

#[repr(C)]
struct FakeContext {
    context_vtbl: *const ContextVtbl,
    insert_vtbl: *const InsertAtSelectionVtbl,
    refs: AtomicU32,
    request_fails: bool,
    defer_preedit: bool,
    queued: Vec<(u32, Raw)>,
    insertion_fails: bool,
    writes: Vec<Vec<u16>>,
}

static CONTEXT_VTBL: ContextVtbl = ContextVtbl {
    base: UnknownVtbl {
        query: context_query,
        add_ref: context_add_ref,
        release: context_release,
    },
    request_edit_session: request_edit,
};
static INSERT_VTBL: InsertAtSelectionVtbl = InsertAtSelectionVtbl {
    base: UnknownVtbl {
        query: insert_query,
        add_ref: insert_add_ref,
        release: insert_release,
    },
    insert_text,
};

unsafe fn insert_owner(this: Raw) -> Raw {
    (this as *mut u8)
        .sub(offset_of!(FakeContext, insert_vtbl))
        .cast()
}
unsafe extern "system" fn context_query(this: Raw, iid: *const Guid, out: *mut Raw) -> HResult {
    *out = ptr::null_mut();
    if *iid == ffi::IID_IUnknown {
        *out = this;
    } else if *iid == ffi::IID_ITfInsertAtSelection {
        *out = ptr::addr_of_mut!((*(this as *mut FakeContext)).insert_vtbl).cast();
    } else {
        return ffi::E_NOINTERFACE;
    }
    context_add_ref(this);
    ffi::S_OK
}
unsafe extern "system" fn context_add_ref(this: Raw) -> u32 {
    (*(this as *mut FakeContext))
        .refs
        .fetch_add(1, Ordering::SeqCst)
        + 1
}
unsafe extern "system" fn context_release(this: Raw) -> u32 {
    (*(this as *mut FakeContext))
        .refs
        .fetch_sub(1, Ordering::SeqCst)
        - 1
}
unsafe extern "system" fn insert_query(this: Raw, iid: *const Guid, out: *mut Raw) -> HResult {
    context_query(insert_owner(this), iid, out)
}
unsafe extern "system" fn insert_add_ref(this: Raw) -> u32 {
    context_add_ref(insert_owner(this))
}
unsafe extern "system" fn insert_release(this: Raw) -> u32 {
    context_release(insert_owner(this))
}
unsafe extern "system" fn request_edit(
    this: Raw,
    id: u32,
    edit: Raw,
    flags: u32,
    result: *mut HResult,
) -> HResult {
    assert_eq!(id, 73);
    let context = &mut *(this as *mut FakeContext);
    if context.defer_preedit {
        ffi::add_ref(edit);
        context.queued.push((flags, edit));
        *result = ffi::TF_S_ASYNC;
        return if flags & ffi::TF_ES_SYNC != 0 {
            ffi::E_UNEXPECTED
        } else {
            ffi::S_OK
        };
    }
    assert!(flags == ffi::TF_ES_SYNC | ffi::TF_ES_READWRITE || flags == ffi::TF_ES_SYNC | 0x2);
    if context.request_fails {
        return ffi::E_FAIL;
    }
    *result = (ffi::vtable::<EditSessionVtbl>(edit).do_edit_session)(edit, 42);
    ffi::S_OK
}
unsafe extern "system" fn insert_text(
    this: Raw,
    cookie: u32,
    flags: u32,
    text: *const u16,
    len: i32,
    range: *mut Raw,
) -> HResult {
    assert_eq!(cookie, 42);
    assert_eq!(flags, 0);
    if range.is_null() {
        return 0x80070057u32 as i32;
    }
    let context = &mut *(insert_owner(this) as *mut FakeContext);
    if context.insertion_fails {
        return ffi::E_FAIL;
    }
    context
        .writes
        .push(std::slice::from_raw_parts(text, len as usize).to_vec());
    ffi::add_ref(this);
    *range = this;
    ffi::S_OK
}

#[repr(C)]
struct ThreadMgrVtbl {
    base: UnknownVtbl,
    activate: unsafe extern "system" fn(Raw, *mut u32) -> HResult,
    deactivate: unsafe extern "system" fn(Raw) -> HResult,
}

#[repr(C)]
struct ClientIdVtbl {
    base: UnknownVtbl,
    get_client_id: unsafe extern "system" fn(Raw, *const Guid, *mut u32) -> HResult,
}

#[link(name = "uuid", kind = "static", modifiers = "-bundle")]
extern "system" {
    static CLSID_TF_ThreadMgr: Guid;
    static IID_ITfThreadMgr: Guid;
    static IID_ITfClientId: Guid;
}

#[test]
fn text_service_lifecycle_and_native_tsf_manager() {
    let _keyboard = TestKeyboardState::neutral();
    let directory = fixture_dir("service-test");
    std::fs::write(directory.join("cangjie3.dict.yaml"), "---\n...\n日\ta\n").unwrap();
    std::fs::write(directory.join("cangjie3_custom.dict.yaml"), "---\n...\n").unwrap();
    crate::tsf::service::set_test_dictionary(directory.clone());
    unsafe {
        assert_eq!(DllCanUnloadNow(), ffi::S_OK);
        let mut factory = ptr::null_mut();
        let wrong_clsid = Guid::new(0, 0, 0, [0; 8]);
        assert_eq!(
            DllGetClassObject(&wrong_clsid, &ffi::IID_IClassFactory, &mut factory),
            ffi::CLASS_E_CLASSNOTAVAILABLE
        );
        assert!(factory.is_null());
        assert_eq!(
            DllGetClassObject(&CLSID, &ffi::IID_IClassFactory, &mut factory),
            ffi::S_OK
        );
        assert_eq!(DllCanUnloadNow(), ffi::S_FALSE);

        let factory_vtbl = ffi::vtable::<ClassFactoryVtbl>(factory);
        let mut processor = ptr::null_mut();
        assert_eq!(
            (factory_vtbl.create_instance)(
                factory,
                factory,
                &ffi::IID_ITfTextInputProcessor,
                &mut processor
            ),
            ffi::CLASS_E_NOAGGREGATION
        );
        assert!(processor.is_null());
        assert_eq!(
            (factory_vtbl.create_instance)(
                factory,
                ptr::null_mut(),
                &ffi::IID_ITfTextInputProcessor,
                &mut processor
            ),
            ffi::S_OK
        );
        let sink = ffi::query(processor, &ffi::IID_ITfKeyEventSink).unwrap();
        let identity = ffi::query(sink, &ffi::IID_IUnknown).unwrap();
        assert_eq!(identity, processor);
        ffi::release(identity);
        let composition_sink = ffi::query(processor, &ffi::IID_ITfCompositionSink).unwrap();
        let identity = ffi::query(composition_sink, &ffi::IID_IUnknown).unwrap();
        assert_eq!(identity, processor);
        ffi::release(identity);
        ffi::release(composition_sink);
        let proc_vtbl = ffi::vtable::<TextInputProcessorVtbl>(processor);
        let sink_vtbl = ffi::vtable::<KeyEventSinkVtbl>(sink);

        let mut manager = FakeManager {
            vtbl: &MANAGER_VTBL,
            refs: AtomicU32::new(1),
            sink: ptr::null_mut(),
            id: 0,
        };
        let manager_ptr: Raw = (&mut manager as *mut FakeManager).cast();
        assert_eq!((proc_vtbl.activate)(processor, manager_ptr, 73), ffi::S_OK);
        assert_eq!(manager.sink, sink);
        assert_eq!(
            (proc_vtbl.activate)(processor, manager_ptr, 73),
            ffi::E_UNEXPECTED
        );

        let mut context = FakeContext {
            context_vtbl: &CONTEXT_VTBL,
            insert_vtbl: &INSERT_VTBL,
            refs: AtomicU32::new(1),
            request_fails: true,
            defer_preedit: false,
            queued: Vec::new(),
            insertion_fails: false,
            writes: Vec::new(),
        };
        let context_ptr: Raw = (&mut context as *mut FakeContext).cast();
        let mut eaten = -1;
        assert_eq!(
            (sink_vtbl.on_test_key_down)(sink, context_ptr, 0x41, 0, &mut eaten),
            ffi::S_OK
        );
        assert_eq!(eaten, 0);
        // Shift down 交給應用程式；單獨釋放時切換中文，release 仍須交還。
        assert_eq!(
            (sink_vtbl.on_test_key_down)(sink, context_ptr, 0x10, 0, &mut eaten),
            ffi::S_OK
        );
        assert_eq!(eaten, 0);
        assert_eq!(
            (sink_vtbl.on_test_key_up)(sink, context_ptr, 0x10, 0, &mut eaten),
            ffi::S_OK
        );
        assert_eq!(eaten, 1);
        assert_eq!(
            (sink_vtbl.on_key_up)(sink, context_ptr, 0x10, 0, &mut eaten),
            ffi::S_OK
        );
        assert_eq!(eaten, 0);
        assert_eq!(
            (sink_vtbl.on_test_key_down)(sink, context_ptr, 0x41, 0, &mut eaten),
            ffi::S_OK
        );
        assert_eq!(eaten, 1);
        // 拿不到編輯鎖時不吞字，也不留下 context 的參考。
        assert_eq!(
            (sink_vtbl.on_key_down)(sink, context_ptr, 0x41, 0, &mut eaten),
            ffi::S_OK
        );
        assert_eq!(eaten, 0);
        assert!(context.writes.is_empty());
        assert_eq!(context.refs.load(Ordering::SeqCst), 1);
        // 同步要求回傳 E_UNEXPECTED / TF_S_ASYNC 時只接受明確重試。
        context.defer_preedit = true;
        assert_eq!(
            (sink_vtbl.on_key_down)(sink, context_ptr, 0x41, 0, &mut eaten),
            ffi::S_OK
        );
        assert_eq!(eaten, 1);
        assert_eq!(context.queued.len(), 2);
        let (sync_flags, sync_edit) = context.queued[0];
        let (async_flags, async_edit) = context.queued[1];
        assert_eq!(sync_flags, ffi::TF_ES_SYNC | ffi::TF_ES_READWRITE);
        assert_eq!(async_flags, ffi::TF_ES_ASYNC | ffi::TF_ES_READWRITE);
        assert_eq!(
            (ffi::vtable::<EditSessionVtbl>(sync_edit).do_edit_session)(sync_edit, 42),
            ffi::S_OK
        );
        ffi::release(sync_edit);
        // 已切走的 context 上收到延遲回呼，也不可以寫入文件。
        assert_eq!((sink_vtbl.on_set_focus)(sink, 0), ffi::S_OK);
        assert_eq!(
            (ffi::vtable::<EditSessionVtbl>(async_edit).do_edit_session)(async_edit, 42),
            ffi::S_OK
        );
        ffi::release(async_edit);
        context.queued.clear();
        context.defer_preedit = false;
        assert!(context.writes.is_empty());
        assert_eq!(context.refs.load(Ordering::SeqCst), 1);
        // 有些宿主只送 OnKeyDown / OnKeyUp；組合鍵不應誤切換中英。
        assert_eq!(
            (sink_vtbl.on_key_down)(sink, context_ptr, 0x10, 0, &mut eaten),
            ffi::S_OK
        );
        assert_eq!(
            (sink_vtbl.on_key_down)(sink, context_ptr, 0x41, 0, &mut eaten),
            ffi::S_OK
        );
        assert_eq!(
            (sink_vtbl.on_key_up)(sink, context_ptr, 0x10, 0, &mut eaten),
            ffi::S_OK
        );
        assert_eq!(
            (sink_vtbl.on_test_key_down)(sink, context_ptr, 0x41, 0, &mut eaten),
            ffi::S_OK
        );
        assert_eq!(eaten, 1);
        // 單獨 Shift 仍須在沒有試探事件時切換。
        assert_eq!(
            (sink_vtbl.on_key_down)(sink, context_ptr, 0x10, 0, &mut eaten),
            ffi::S_OK
        );
        assert_eq!(
            (sink_vtbl.on_key_up)(sink, context_ptr, 0x10, 0, &mut eaten),
            ffi::S_OK
        );
        assert_eq!(
            (sink_vtbl.on_test_key_down)(sink, context_ptr, 0x41, 0, &mut eaten),
            ffi::S_OK
        );
        assert_eq!(eaten, 0);
        assert_eq!((proc_vtbl.deactivate)(processor), ffi::S_OK);
        assert!(manager.sink.is_null());
        assert_eq!(manager.refs.load(Ordering::SeqCst), 1);

        // 另以 Windows 真實 TSF 管理員測試 advise / unadvise，不需註冊輸入法。
        assert!(ffi::CoInitializeEx(ptr::null_mut(), 2) >= 0);
        let real_manager = ffi::create_instance(&CLSID_TF_ThreadMgr, &IID_ITfThreadMgr).unwrap();
        let thread_vtbl = ffi::vtable::<ThreadMgrVtbl>(real_manager.as_raw());
        let mut client_id = 0;
        assert_eq!(
            (thread_vtbl.activate)(real_manager.as_raw(), &mut client_id),
            ffi::S_OK
        );
        assert_ne!(client_id, 0);
        let client_id_mgr = ffi::query(real_manager.as_raw(), &IID_ITfClientId).unwrap();
        let mut service_id = 0;
        assert_eq!(
            (ffi::vtable::<ClientIdVtbl>(client_id_mgr).get_client_id)(
                client_id_mgr,
                &CLSID,
                &mut service_id
            ),
            ffi::S_OK
        );
        assert_ne!(service_id, 0);
        ffi::release(client_id_mgr);
        // 尚未安裝此 CLSID 時，Windows 可能拒絕 advise；成功與失敗都必須可乾淨解除。
        let service_activation = (proc_vtbl.activate)(processor, real_manager.as_raw(), service_id);
        println!(
            "direct activation outside TSF-managed lifetime HRESULT: {:#010x}",
            service_activation as u32
        );
        assert_eq!((proc_vtbl.deactivate)(processor), ffi::S_OK);
        assert_eq!((thread_vtbl.deactivate)(real_manager.as_raw()), ffi::S_OK);
        drop(real_manager);
        ffi::CoUninitialize();

        ffi::release(sink);
        ffi::release(processor);
        ffi::release(factory);
        assert_eq!(DllCanUnloadNow(), ffi::S_OK);
    }
    remove_fixture(&directory);
}
