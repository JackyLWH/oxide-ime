//! 使用原生 TSF composition 更新組字，並在單一編輯工作階段提交文字。
use std::ptr;
use std::sync::Arc;

use super::display_attribute::INPUT_ATTRIBUTE;
use super::ffi::{
    self, CategoryMgrVtbl, ComPtr, CompositionVtbl, ContextCompositionVtbl, ContextFullVtbl,
    ContextViewVtbl, HResult, InsertAtSelectionVtbl, PropertyVtbl, RangeVtbl, Raw, Rect, Selection,
    Variant, VariantData,
};
use crate::diagnostics::{self, Level};
use crate::engine::Edit;
use crate::maintenance::caret::Correction;

const MAX_ORIGINAL_UTF16: usize = 8 * 1024 * 1024;
const MAX_DIAGNOSTIC_UTF16: usize = 32 * 1024;

struct ActiveComposition {
    object: ComPtr,
    context: ComPtr,
    original: Arc<[u16]>,
    display_applied: bool,
}

pub(crate) struct CompositionState {
    active: Option<ActiveComposition>,
    inserted: Option<ComPtr>,
    correction: Option<Correction>,
    display_atom: u32,
}

impl CompositionState {
    pub(crate) fn new() -> Self {
        Self {
            active: None,
            inserted: None,
            correction: None,
            display_atom: 0,
        }
    }

    pub(crate) fn is_active(&self) -> bool {
        self.active.is_some()
    }

    pub(crate) fn context(&self) -> Raw {
        self.active
            .as_ref()
            .map_or(ptr::null_mut(), |active| active.context.as_raw())
    }

    pub(crate) fn original(&self) -> Option<Arc<[u16]>> {
        self.active.as_ref().map(|active| active.original.clone())
    }

    pub(crate) fn take_caret_correction(&mut self) -> Option<Correction> {
        self.inserted.take();
        self.correction.take()
    }

    fn object(&self) -> Raw {
        self.active
            .as_ref()
            .map_or(ptr::null_mut(), |active| active.object.as_raw())
    }
}

unsafe fn selection_range(context: Raw, cookie: u32) -> Result<ComPtr, HResult> {
    let mut selection = Selection {
        range: ptr::null_mut(),
        active_end: 0,
        interim: 0,
    };
    let mut fetched = 0;
    let hr = (ffi::vtable::<ContextFullVtbl>(context).get_selection)(
        context,
        cookie,
        u32::MAX,
        1,
        &mut selection,
        &mut fetched,
    );
    if hr < 0 {
        return Err(hr);
    }
    if fetched != 1 {
        return Err(ffi::E_UNEXPECTED);
    }
    ComPtr::from_out(hr, selection.range)
}

unsafe fn clone_range(range: Raw) -> Result<ComPtr, HResult> {
    let mut result = ptr::null_mut();
    let hr = (ffi::vtable::<RangeVtbl>(range).clone_range)(range, &mut result);
    ComPtr::from_out(hr, result)
}

unsafe fn composition_range(composition: Raw) -> Result<ComPtr, HResult> {
    let mut range = ptr::null_mut();
    let hr = (ffi::vtable::<CompositionVtbl>(composition).get_range)(composition, &mut range);
    ComPtr::from_out(hr, range)
}

unsafe fn read_range(range: Raw, cookie: u32, limit: usize) -> Result<Vec<u16>, HResult> {
    let copy = clone_range(range)?;
    let mut text = Vec::new();
    loop {
        let mut buffer = [0u16; 1024];
        let mut count = 0;
        let hr = (ffi::vtable::<RangeVtbl>(copy.as_raw()).get_text)(
            copy.as_raw(),
            cookie,
            1,
            buffer.as_mut_ptr(),
            buffer.len() as u32,
            &mut count,
        );
        if hr < 0 {
            return Err(hr);
        }
        if count == 0 {
            break;
        }
        if count as usize > buffer.len() || text.len() + count as usize > limit {
            return Err(ffi::E_FAIL);
        }
        text.extend_from_slice(&buffer[..count as usize]);
    }
    Ok(text)
}

unsafe fn following_text(context: Raw, cookie: u32) -> Option<String> {
    let selection = selection_range(context, cookie).ok()?;
    let tail = caret_at_end(selection.as_raw(), cookie)?;
    let raw = tail.as_raw();
    let vtbl = ffi::vtable::<RangeVtbl>(raw);
    let mut movement = 0;
    if (vtbl.shift_end)(raw, cookie, 16, &mut movement, ptr::null_mut()) < 0 {
        return None;
    }
    let mut buffer = [0; 16];
    let mut count = 0;
    let hr = (vtbl.get_text)(
        raw,
        cookie,
        0,
        buffer.as_mut_ptr(),
        buffer.len() as u32,
        &mut count,
    );
    if hr < 0 || count > buffer.len() as u32 {
        return None;
    }
    let mut text = &buffer[..count as usize];
    if text
        .last()
        .is_some_and(|unit| (0xD800..=0xDBFF).contains(unit))
    {
        text = &text[..text.len() - 1];
    }
    Some(
        String::from_utf16(text)
            .ok()?
            .split(['\r', '\n'])
            .next()?
            .to_owned(),
    )
}

fn write_details(text: &[u16]) -> [(&'static str, i64); 5] {
    let mut newlines = 0;
    let mut carriage_returns = 0;
    let mut stops = 0;
    let mut line_stops = 0;
    for (index, &unit) in text.iter().enumerate() {
        match unit {
            0x000a => newlines += 1,
            0x000d => carriage_returns += 1,
            0x3002 => {
                stops += 1;
                if matches!(text.get(index + 1), Some(0x000a | 0x000d)) {
                    line_stops += 1;
                }
            }
            _ => {}
        }
    }
    [
        ("utf16", text.len() as i64),
        ("lf", newlines),
        ("cr", carriage_returns),
        ("ideographic_stops", stops),
        ("stops_before_break", line_stops),
    ]
}

unsafe fn inspect_write(range: Raw, cookie: u32, expected: &[u16], edit_id: u32) {
    if expected.len() > MAX_DIAGNOSTIC_UTF16 {
        return;
    }
    // 只讀本次寫入的範圍。文字留在記憶體，事件只保留比對結果及數量。
    match read_range(range, cookie, MAX_DIAGNOSTIC_UTF16) {
        Ok(actual) => {
            let mut fields = write_details(&actual).to_vec();
            fields.extend([
                ("edit_id", i64::from(edit_id)),
                ("exact", i64::from(actual == expected)),
            ]);
            diagnostics::event(Level::Detail, "edit", "readback", &fields);
        }
        Err(hr) => diagnostics::event(
            Level::Detail,
            "edit",
            "readback_failed",
            &[
                ("edit_id", i64::from(edit_id)),
                ("hresult", diagnostics::hresult(hr)),
            ],
        ),
    }
}

fn trace_write(operation: &'static str, text: &[u16], edit_id: u32, hr: HResult) {
    let mut fields = write_details(text).to_vec();
    fields.extend([
        ("edit_id", i64::from(edit_id)),
        ("hresult", diagnostics::hresult(hr)),
    ]);
    diagnostics::event(Level::Detail, "edit", operation, &fields);
}

unsafe fn set_range_text(range: Raw, cookie: u32, text: &[u16]) -> HResult {
    if text.len() > i32::MAX as usize {
        return ffi::E_FAIL;
    }
    let data = if text.is_empty() {
        ptr::null()
    } else {
        text.as_ptr()
    };
    (ffi::vtable::<RangeVtbl>(range).set_text)(range, cookie, 0, data, text.len() as i32)
}

unsafe fn insert_at_selection(
    context: Raw,
    cookie: u32,
    encoded: &[u16],
    flags: u32,
    trace: Option<u32>,
) -> Result<Option<ComPtr>, HResult> {
    let insert = ComPtr::query(context, &ffi::IID_ITfInsertAtSelection)?;
    let length = i32::try_from(encoded.len()).map_err(|_| ffi::E_FAIL)?;
    let mut range = ptr::null_mut();
    let hr = (ffi::vtable::<InsertAtSelectionVtbl>(insert.as_raw()).insert_text)(
        insert.as_raw(),
        cookie,
        flags,
        encoded.as_ptr(),
        length,
        &mut range,
    );
    if let Some(edit_id) = trace {
        trace_write("insert_at_selection", encoded, edit_id, hr);
    }
    if hr < 0 {
        return Err(hr);
    }
    // 寫入已成功；後續調整插入點失敗時不能把按鍵交回應用程式再輸入一次。
    let range = ComPtr::from_out(hr, range).ok();
    if let Some(range) = range.as_ref() {
        if let Some(edit_id) = trace {
            inspect_write(range.as_raw(), cookie, encoded, edit_id);
        }
        if let Some(caret) = caret_at_end(range.as_raw(), cookie) {
            let hr = set_caret(context, cookie, caret.as_raw());
            if hr < 0 {
                diagnostics::result("edit", "set_inserted_caret", hr);
            }
        }
    }
    Ok(range)
}

unsafe fn attribute_property(context: Raw) -> Result<ComPtr, HResult> {
    let mut property = ptr::null_mut();
    let hr = (ffi::vtable::<ContextFullVtbl>(context).get_property)(
        context,
        &ffi::GUID_PROP_ATTRIBUTE,
        &mut property,
    );
    ComPtr::from_out(hr, property)
}

unsafe fn apply_display_attribute(
    state: *mut CompositionState,
    object: Raw,
    context: Raw,
    cookie: u32,
    range: Raw,
) {
    if (*state).display_atom == 0 {
        let Ok(manager) =
            ffi::create_instance(&ffi::CLSID_TF_CategoryMgr, &ffi::IID_ITfCategoryMgr)
        else {
            return;
        };
        let mut atom = 0;
        if (ffi::vtable::<CategoryMgrVtbl>(manager.as_raw()).register_guid)(
            manager.as_raw(),
            &INPUT_ATTRIBUTE,
            &mut atom,
        ) >= 0
        {
            (*state).display_atom = atom;
        }
    }
    if (*state).display_atom == 0 {
        return;
    }
    if let Ok(property) = attribute_property(context) {
        let value = Variant {
            kind: 3,
            reserved: [0; 3],
            data: VariantData {
                storage: [(*state).display_atom as usize, 0],
            },
        }; // VT_I4
        if (ffi::vtable::<PropertyVtbl>(property.as_raw()).set_value)(
            property.as_raw(),
            cookie,
            range,
            &value,
        ) >= 0
        {
            if let Some(active) = (*state)
                .active
                .as_mut()
                .filter(|active| active.object.as_raw() == object)
            {
                active.display_applied = true;
            }
        }
    }
}

unsafe fn clear_display_attribute(context: Raw, cookie: u32, range: Raw) {
    if let Ok(property) = attribute_property(context) {
        let _ = (ffi::vtable::<PropertyVtbl>(property.as_raw()).clear)(
            property.as_raw(),
            cookie,
            range,
        );
    }
}

unsafe fn caret_at_end(range: Raw, cookie: u32) -> Option<ComPtr> {
    let caret = clone_range(range).ok()?;
    let hr = (ffi::vtable::<RangeVtbl>(caret.as_raw()).collapse)(caret.as_raw(), cookie, 1); // TF_ANCHOR_END
    (hr >= 0).then_some(caret)
}

unsafe fn set_caret(context: Raw, cookie: u32, caret: Raw) -> HResult {
    let selection = Selection {
        range: caret,
        active_end: 0,
        interim: 0,
    };
    (ffi::vtable::<ContextFullVtbl>(context).set_selection)(context, cookie, 1, &selection)
}

unsafe fn confirm_multiline_caret(context: Raw, cookie: u32, inserted: Raw) -> Result<(), HResult> {
    let selection = selection_range(context, cookie)?;
    let raw = selection.as_raw();
    let vtbl = ffi::vtable::<RangeVtbl>(raw);
    let mut collapsed = 0;
    let mut start = 0;
    let mut end = 0;
    if (vtbl.is_empty)(raw, cookie, &mut collapsed) < 0
        || collapsed == 0
        || (vtbl.compare_start)(raw, cookie, inserted, 0, &mut start) < 0
        || (vtbl.compare_start)(raw, cookie, inserted, 1, &mut end) < 0
        || start < 0
        || end >= 0
    {
        return Ok(());
    }
    // 有些宿主在前一次編輯鎖釋放後才更新游標。只修正仍在本次
    // 插入範圍內的單一插入點，保留宿主在範圍之外的新選取。
    let caret = caret_at_end(inserted, cookie).ok_or(ffi::E_FAIL)?;
    let hr = set_caret(context, cookie, caret.as_raw());
    diagnostics::result("edit", "confirm_inserted_caret", hr);
    if hr < 0 {
        Err(hr)
    } else {
        Ok(())
    }
}

unsafe fn text_anchor(context: Raw, cookie: u32, caret: Raw) -> Option<Rect> {
    let mut view = ptr::null_mut();
    let hr = (ffi::vtable::<ContextFullVtbl>(context).get_active_view)(context, &mut view);
    let view = ComPtr::from_out(hr, view).ok()?;
    let mut rect = Rect::default();
    let mut clipped = 0;
    let hr = (ffi::vtable::<ContextViewVtbl>(view.as_raw()).get_text_ext)(
        view.as_raw(),
        cookie,
        caret,
        &mut rect,
        &mut clipped,
    );
    (hr >= 0 && rect != Rect::default()).then_some(rect)
}

unsafe fn caret_and_anchor(context: Raw, cookie: u32, range: Raw) -> Option<Rect> {
    let caret = caret_at_end(range, cookie)?;
    set_caret(context, cookie, caret.as_raw());
    text_anchor(context, cookie, caret.as_raw())
}

unsafe fn start(
    state: *mut CompositionState,
    context: Raw,
    cookie: u32,
    sink: Raw,
) -> Result<(), HResult> {
    let range = selection_range(context, cookie)?;
    let original = read_range(range.as_raw(), cookie, MAX_ORIGINAL_UTF16)?;
    let manager = ComPtr::query(context, &ffi::IID_ITfContextComposition)?;
    let mut composition = ptr::null_mut();
    let hr = (ffi::vtable::<ContextCompositionVtbl>(manager.as_raw()).start)(
        manager.as_raw(),
        cookie,
        range.as_raw(),
        sink,
        &mut composition,
    );
    (*state).active = Some(ActiveComposition {
        object: ComPtr::from_out(hr, composition)?,
        context: ComPtr::retain(context),
        original: original.into(),
        display_applied: false,
    });
    Ok(())
}

unsafe fn finish(state: *mut CompositionState, object: Raw, cookie: u32) {
    if (*state).object() == object {
        // 先取走組字狀態，避免 TSF 同步結束回呼再次還原原文。
        if let Some(active) = (*state).active.take() {
            let hr = (ffi::vtable::<CompositionVtbl>(active.object.as_raw()).end)(
                active.object.as_raw(),
                cookie,
            );
            if hr < 0 {
                diagnostics::result("edit", "end_composition", hr);
            }
        }
    }
}

unsafe fn rewrite_and_finish(
    state: *mut CompositionState,
    context: Raw,
    cookie: u32,
    text: &[u16],
    move_caret: bool,
    trace: Option<u32>,
) -> Result<Option<Rect>, HResult> {
    let object = (*state).object();
    let range = composition_range(object)?;
    let hr = set_range_text(range.as_raw(), cookie, text);
    if let Some(edit_id) = trace {
        trace_write("set_text", text, edit_id, hr);
    }
    if hr < 0 {
        return Err(hr);
    }
    // SetText 可同步觸發宿主終止組字。只處理本次寫入所屬的範圍。
    let mut anchor = None;
    if let Some(applied) = (*state)
        .active
        .as_ref()
        .filter(|active| active.object.as_raw() == object)
        .map(|active| active.display_applied)
    {
        if applied || move_caret || trace.is_some() {
            if let Ok(updated) = composition_range(object) {
                if let Some(edit_id) = trace {
                    inspect_write(updated.as_raw(), cookie, text, edit_id);
                }
                if applied {
                    clear_display_attribute(context, cookie, updated.as_raw());
                }
                if move_caret {
                    anchor = caret_and_anchor(context, cookie, updated.as_raw());
                }
            }
        }
        finish(state, object, cookie);
    }
    Ok(anchor)
}

/// `state` 由同一 TSF apartment 的文字服務擁有；不跨 COM 呼叫保留 Rust 可變參考。
pub(crate) unsafe fn apply(
    state: *mut CompositionState,
    context: Raw,
    cookie: u32,
    sink: Raw,
    edit: &Edit,
    trace_id: u32,
) -> Result<Option<Rect>, HResult> {
    if context.is_null() {
        return Err(ffi::E_POINTER);
    }
    if (*state).is_active() && (*state).context() != context {
        return Err(ffi::E_UNEXPECTED);
    }
    match edit {
        Edit::None => Ok(None),
        Edit::Inspect => Ok(selection_range(context, cookie)
            .ok()
            .and_then(|range| caret_at_end(range.as_raw(), cookie))
            .and_then(|caret| text_anchor(context, cookie, caret.as_raw()))),
        Edit::Insert(text) => {
            if (*state).is_active() {
                return Err(ffi::E_UNEXPECTED);
            }
            let encoded: Vec<u16> = text.encode_utf16().collect();
            let flags = if text.contains(['\n', '\r']) {
                // 瀏覽器在這裡重新建立預設組字，可能令富文字編輯器重複套用多行內容。
                ffi::TF_IAS_NO_DEFAULT_COMPOSITION
            } else {
                0
            };
            let correction = if flags == ffi::TF_IAS_NO_DEFAULT_COMPOSITION {
                Correction::prepare(text).and_then(|mut correction| {
                    correction.following(following_text(context, cookie)?);
                    Some(correction)
                })
            } else {
                None
            };
            let inserted = insert_at_selection(
                context,
                cookie,
                &encoded,
                flags,
                diagnostics::detailed().then_some(trace_id),
            )?;
            if flags == ffi::TF_IAS_NO_DEFAULT_COMPOSITION && !(*state).is_active() {
                (*state).inserted = inserted;
                (*state).correction = correction;
            }
            Ok(None)
        }
        Edit::ConfirmMultilineCaret => {
            if let Some(inserted) = (*state).inserted.take().filter(|_| !(*state).is_active()) {
                confirm_multiline_caret(context, cookie, inserted.as_raw())?;
            }
            Ok(None)
        }
        Edit::RestoreSelection(original) => {
            if (*state).is_active() {
                return Err(ffi::E_UNEXPECTED);
            }
            insert_at_selection(
                context,
                cookie,
                original,
                ffi::TF_IAS_NO_DEFAULT_COMPOSITION,
                None,
            )
            .map(|_| None)
        }
        Edit::Preedit(text) => {
            let fresh = !(*state).is_active();
            if fresh {
                start(state, context, cookie, sink)?;
            }
            let object = (*state).object();
            let range = match composition_range(object) {
                Ok(range) => range,
                Err(hr) => {
                    if fresh {
                        finish(state, object, cookie);
                    }
                    return Err(hr);
                }
            };
            let encoded: Vec<u16> = text.encode_utf16().collect();
            let hr = set_range_text(range.as_raw(), cookie, &encoded);
            if hr < 0 {
                if fresh {
                    finish(state, object, cookie);
                }
                return Err(hr);
            }
            if (*state).object() == object {
                if let Ok(updated_range) = composition_range(object) {
                    // 屬性只影響顯示；已寫入的字根仍然只送出一次。
                    apply_display_attribute(state, object, context, cookie, updated_range.as_raw());
                    return Ok(caret_and_anchor(context, cookie, updated_range.as_raw()));
                }
            }
            Ok(None)
        }
        Edit::Commit(text) | Edit::CommitCandidate(text) => {
            if !(*state).is_active() {
                return Err(ffi::E_UNEXPECTED);
            }
            let encoded: Vec<u16> = text.encode_utf16().collect();
            let trace = diagnostics::detailed().then_some(trace_id);
            rewrite_and_finish(state, context, cookie, &encoded, true, trace)
        }
        Edit::PrepareMultiline => {
            if !(*state).is_active() {
                return Err(ffi::E_UNEXPECTED);
            }
            rewrite_and_finish(state, context, cookie, &[], true, None)
        }
        Edit::Cancel | Edit::CancelExternal => {
            let Some(original) = (*state)
                .active
                .as_ref()
                .map(|active| active.original.clone())
            else {
                return Ok(None);
            };
            rewrite_and_finish(
                state,
                context,
                cookie,
                &original,
                *edit == Edit::Cancel,
                None,
            )
        }
    }
}

/// 應用程式終止 composition 時，在其寫入鎖內還原原文並清除本服務的參考。
pub(crate) unsafe fn terminated(
    state: *mut CompositionState,
    composition: Raw,
    cookie: u32,
) -> bool {
    if composition.is_null() || (*state).object() != composition {
        return false;
    }
    let Some(active) = (*state).active.take() else {
        return false;
    };
    if let Ok(range) = composition_range(active.object.as_raw()) {
        let hr = set_range_text(range.as_raw(), cookie, &active.original);
        diagnostics::event(
            if hr < 0 {
                Level::Warning
            } else {
                Level::Detail
            },
            "edit",
            "host_terminated_composition",
            &[("hresult", diagnostics::hresult(hr))],
        );
        if active.display_applied {
            clear_display_attribute(active.context.as_raw(), cookie, range.as_raw());
        }
    }
    true
}

/// 無法取得編輯鎖時放棄本服務的持有參考；TSF 仍可自行終止剩餘組字。
pub(crate) unsafe fn abandon(state: *mut CompositionState) {
    diagnostics::event(Level::Warning, "edit", "abandon_composition", &[]);
    drop((*state).active.take());
}
