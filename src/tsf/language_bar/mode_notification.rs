//! 用 TSF 全域 compartment 通知同一桌面的輸入指示器重新讀取共用模式。
use std::ptr;

use crate::diagnostics::{self, Level};
use crate::tsf::ffi::{
    self, ComPtr, CompartmentMgrVtbl, CompartmentVtbl, Guid, HResult, Raw, SourceVtbl,
    ThreadManagerVtbl, Variant, VariantData,
};

pub(super) const MODE_CHANGED: Guid = Guid::new(
    0x10e80b1b,
    0xcacd,
    0x4611,
    [0xb9, 0xd0, 0xca, 0xee, 0x23, 0x07, 0xff, 0x58],
);

pub(super) struct ModeNotification {
    compartment: ComPtr,
    subscription: Option<(ComPtr, u32)>,
}

impl ModeNotification {
    pub(super) unsafe fn open(manager: Raw) -> Result<Self, HResult> {
        let thread = ComPtr::query(manager, &ffi::IID_ITfThreadMgr)?;
        let mut global = ptr::null_mut();
        let hr = (ffi::vtable::<ThreadManagerVtbl>(thread.as_raw()).get_global_compartment)(
            thread.as_raw(),
            &mut global,
        );
        let global = ComPtr::from_out(hr, global)?;
        let mut compartment = ptr::null_mut();
        let hr = (ffi::vtable::<CompartmentMgrVtbl>(global.as_raw()).get_compartment)(
            global.as_raw(),
            &MODE_CHANGED,
            &mut compartment,
        );
        Ok(Self {
            compartment: ComPtr::from_out(hr, compartment)?,
            subscription: None,
        })
    }

    pub(super) unsafe fn subscribe(&mut self, sink: Raw) -> Result<(), HResult> {
        let source = ComPtr::query(self.compartment.as_raw(), &ffi::IID_ITfSource)?;
        let mut cookie = 0;
        let hr = (ffi::vtable::<SourceVtbl>(source.as_raw()).advise)(
            source.as_raw(),
            &ffi::IID_ITfCompartmentEventSink,
            sink,
            &mut cookie,
        );
        if hr < 0 {
            return Err(hr);
        }
        self.subscription = Some((source, cookie));
        Ok(())
    }

    pub(super) unsafe fn publish(&self, client: u32, ascii: bool) {
        // Compartment 只作通知。接收者以字庫服務的最新狀態為準。
        let value = Variant {
            kind: 3, // VT_I4
            reserved: [0; 3],
            data: VariantData {
                storage: [usize::from(ascii), 0],
            },
        };
        let channel = self.compartment.as_raw();
        let hr = (ffi::vtable::<CompartmentVtbl>(channel).set_value)(channel, client, &value);
        diagnostics::event(
            Level::Detail,
            "language_bar",
            "mode_notify",
            &[("hresult", diagnostics::hresult(hr))],
        );
    }
}

impl Drop for ModeNotification {
    fn drop(&mut self) {
        if let Some((source, cookie)) = self.subscription.take() {
            unsafe {
                (ffi::vtable::<SourceVtbl>(source.as_raw()).unadvise)(source.as_raw(), cookie);
            }
        }
    }
}
