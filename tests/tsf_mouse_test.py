"""隔離桌面、真實 TSF：滑鼠取消組字及點選完整候選，不操作使用者桌面。"""
import ctypes as c
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import threading
import time
import uuid

sys.dont_write_bytecode = True
import tsf_real_store_test as t

u = c.WinDLL("user32", use_last_error=True)
k = c.WinDLL("kernel32", use_last_error=True)
k.FreeLibrary.argtypes = [t.PTR]
ole = c.WinDLL("ole32")
u.CreateDesktopW.argtypes = [c.c_wchar_p, t.PTR, t.PTR, t.DWORD, t.DWORD, t.PTR]
u.CreateDesktopW.restype = t.PTR
u.SetThreadDesktop.argtypes = [t.PTR]
u.CloseDesktop.argtypes = [t.PTR]
u.CreateWindowExW.argtypes = [t.DWORD, c.c_wchar_p, c.c_wchar_p, t.DWORD, t.LONG, t.LONG,
                             t.LONG, t.LONG, t.PTR, t.PTR, t.PTR, t.PTR]
u.CreateWindowExW.restype = t.PTR
u.SetFocus.argtypes = [t.PTR]
u.SetFocus.restype = t.PTR
u.GetFocus.restype = t.PTR
u.DestroyWindow.argtypes = [t.PTR]
u.PostMessageW.argtypes = [t.PTR, t.DWORD, c.c_size_t, c.c_ssize_t]
u.GetWindowTextW.argtypes = [t.PTR, c.c_wchar_p, c.c_int]
u.IsWindowVisible.argtypes = [t.PTR]
u.GetKeyboardState.argtypes = [c.POINTER(c.c_ubyte)]
u.SetKeyboardState.argtypes = [c.POINTER(c.c_ubyte)]
callback_type = c.WINFUNCTYPE(c.c_int, t.PTR, c.c_ssize_t)
u.EnumThreadWindows.argtypes = [t.DWORD, callback_type, c.c_ssize_t]
ole.CoInitializeEx.argtypes = [t.PTR, t.DWORD]
ole.CoInitializeEx.restype = t.HR
ole.CoCreateInstance.argtypes = [c.POINTER(t.Guid), t.PTR, t.DWORD, c.POINTER(t.Guid), t.PPTR]
ole.CoCreateInstance.restype = t.HR
ole.CoRegisterClassObject.argtypes = [c.POINTER(t.Guid), t.PTR, t.DWORD, t.DWORD, c.POINTER(t.DWORD)]
ole.CoRegisterClassObject.restype = t.HR


class Point(c.Structure):
    _fields_ = [("x", t.LONG), ("y", t.LONG)]


class Rect(c.Structure):
    _fields_ = [("left", t.LONG), ("top", t.LONG), ("right", t.LONG), ("bottom", t.LONG)]


u.GetWindowRect.argtypes = [t.PTR, c.POINTER(Rect)]
u.WindowFromPoint.argtypes = [Point]
u.WindowFromPoint.restype = t.PTR


class Message(c.Structure):
    _fields_ = [("window", t.PTR), ("message", t.DWORD), ("wparam", c.c_size_t),
                ("lparam", c.c_ssize_t), ("time", t.DWORD), ("point", Point), ("private", t.DWORD)]


u.PeekMessageW.argtypes = [c.POINTER(Message), t.PTR, t.DWORD, t.DWORD, t.DWORD]
u.DispatchMessageW.argtypes = [c.POINTER(Message)]


class FakeKeyManager:
    """隔離桌面不載入目前使用者 TIP；編輯內容仍使用真正的 TSF context。"""

    def __init__(self, manager=None):
        self.refs = 1
        self.manager = manager
        self.callbacks = [
            t.WIN(t.HR, t.PTR, c.POINTER(t.Guid), t.PPTR)(self.query),
            t.WIN(t.ULONG, t.PTR)(self.add_ref),
            t.WIN(t.ULONG, t.PTR)(self.release),
            t.WIN(t.HR, t.PTR, t.DWORD, t.PTR, t.LONG)(self.advise),
            t.WIN(t.HR, t.PTR, t.DWORD)(self.unadvise),
        ]
        self.table = (t.PTR * len(self.callbacks))(*[c.cast(fn, t.PTR).value for fn in self.callbacks])
        self.object = t.Obj(c.cast(self.table, c.POINTER(t.PTR)))
        self.ptr = c.cast(c.pointer(self.object), t.PTR)

    def query(self, this, iid, out):
        if self.manager and bytes(iid.contents) == bytes(t.guid("ea1ea135-19df-11d7-a6d2-00065b84435c")):
            hr, result = t.query(self.manager, iid.contents)
            out[0] = result.value if result else None
            return hr
        if bytes(iid.contents) not in (bytes(t.IUNKNOWN), bytes(t.guid("aa80e7f0-2021-11d2-93e0-0060b067b86e"))):
            out[0] = None
            return t.E_NOINTERFACE
        out[0] = this
        self.refs += 1
        return 0

    def add_ref(self, this):
        self.refs += 1
        return self.refs

    def release(self, this):
        self.refs -= 1
        return self.refs

    def advise(self, this, client, sink, foreground):
        return 0

    def unadvise(self, this, client):
        return 0


def worker(dll_path, ui_less=False):
    desktop = u.CreateDesktopW(f"rust-ime-mouse-{uuid.uuid4()}", None, None, 0, 0xF01FF, None)
    if not desktop or not u.SetThreadDesktop(desktop):
        raise OSError(c.get_last_error(), "無法建立隔離桌面")
    t.checked("CoInitializeEx", ole.CoInitializeEx(None, 2))
    dll = c.WinDLL(str(dll_path))
    dll.DllGetClassObject.argtypes = [c.POINTER(t.Guid), c.POINTER(t.Guid), t.PPTR]
    dll.DllGetClassObject.restype = t.HR
    factory = t.PTR()
    t.checked("Class factory", dll.DllGetClassObject(c.byref(t.OUR_CLSID), c.byref(t.ICLASSFACTORY), c.byref(factory)))
    manager = t.create_com(ole, "529a9e6b-6587-4f23-ab9e-9c7d683e3c50", "aa80e801-2021-11d2-93e0-0060b067b86e")
    client = t.DWORD()
    t.checked("Activate thread", t.call(manager, 3, [c.POINTER(t.DWORD)], c.byref(client)))
    document = t.PTR()
    t.checked("Document", t.call(manager, 5, [t.PPTR], c.byref(document)))
    store = t.MemoryTextStore()
    context, cookie = t.PTR(), t.DWORD()
    t.checked("Context", t.call(document, 3, [t.DWORD, t.DWORD, t.PTR, t.PPTR, c.POINTER(t.DWORD)],
                                client.value, 0, store.ptr, c.byref(context), c.byref(cookie)))
    t.checked("Push", t.call(document, 4, [t.PTR], context))
    # 測試宿主也必須能接收真正的滑鼠命中；STATIC 預設會穿透滑鼠。
    hwnd = u.CreateWindowExW(0x08000000, "STATIC", "隔離測試", 0x90000100,
                             100, 100, 300, 100, None, None, None, None)
    if not hwnd:
        raise OSError(c.get_last_error(), "無法建立隔離測試視窗")
    previous = t.PTR()
    t.checked("Associate focus", t.call(manager, 9, [t.PTR, t.PTR, t.PPTR], hwnd, document, c.byref(previous)))
    t.release(previous)
    u.SetFocus(hwnd)
    if u.GetFocus() != hwnd:
        raise RuntimeError("隔離視窗未取得執行緒焦點")
    t.checked("Set document focus", t.call(manager, 8, [t.PTR], document))
    processor = t.PTR()
    t.checked("Create text service", t.call(factory, 3, [t.PTR, c.POINTER(t.Guid), t.PPTR],
                                              None, c.byref(t.ITEXTINPUTPROCESSOR), c.byref(processor)))
    fake_keys = FakeKeyManager(manager if ui_less else None)
    source = None
    ui_sink = None
    sink_cookie = t.DWORD()
    if ui_less:
        hr, source = t.query(manager, t.guid("4ea48a35-60ae-446f-8fd6-e6a8d82459f7"))
        t.checked("Query source", hr)
        ui_sink = t.UiElementSink()
        ui_sink_id = t.guid("ea1ea136-19df-11d7-a6d2-00065b84435c")
        t.checked("Advise UI sink", t.call(source, 3, [c.POINTER(t.Guid), t.PTR, c.POINTER(t.DWORD)],
                                            c.byref(ui_sink_id), ui_sink.ptr, c.byref(sink_cookie)))
        hr, processor_ex = t.query(processor, t.guid("6e4e2102-f9cd-433d-b496-303ce03a6507"))
        t.checked("Query processor Ex", hr)
        t.checked("Activate UI-less", t.call(processor_ex, 5, [t.PTR, t.DWORD, t.DWORD], fake_keys.ptr, client.value, 4))
        t.release(processor_ex)
    else:
        t.checked("Activate text service", t.call(processor, 3, [t.PTR, t.DWORD], fake_keys.ptr, client.value))
    hr, sink = t.query(processor, t.IKEYEVENTSINK)
    t.checked("Key sink", hr)

    def event(slot, key):
        eaten = t.LONG()
        t.checked("key", t.call(sink, slot, [t.PTR, c.c_size_t, c.c_ssize_t, c.POINTER(t.LONG)],
                                  context, key, 0, c.byref(eaten)))
        return eaten.value

    def press(key):
        if event(4, key) != 1 or event(6, key) != 1:
            raise RuntimeError(f"按鍵未處理：{key:#x}")
        if event(5, key):
            event(7, key)

    def ctrl_slash():
        keyboard_type = c.c_ubyte * 256
        saved = keyboard_type()
        if not u.GetKeyboardState(saved):
            raise c.WinError(c.get_last_error())
        modified = keyboard_type(*saved)
        modified[0x11] |= 0x80
        if not u.SetKeyboardState(modified):
            raise c.WinError(c.get_last_error())
        try:
            press(0xbf)
        finally:
            if not u.SetKeyboardState(saved):
                raise c.WinError(c.get_last_error())

    def click(window, x, y):
        rect = Rect()
        if not u.GetWindowRect(window, c.byref(rect)):
            raise OSError(c.get_last_error(), "無法取得隔離視窗座標")
        target = u.WindowFromPoint(Point(rect.left + x, rect.top + y))
        if target != window:
            raise RuntimeError("Windows 滑鼠命中測試未指向要點擊的視窗")
        if not u.PostMessageW(window, 0x201, 0, (y << 16) | x):
            raise OSError(c.get_last_error(), "無法傳送隔離滑鼠測試")
        message = Message()
        if not u.PeekMessageW(c.byref(message), window, 0x201, 0x201, 1):
            raise RuntimeError("無法取得隔離滑鼠訊息")
        u.DispatchMessageW(c.byref(message))

    def candidate_window():
        found = []

        @callback_type
        def visit(window, unused):
            name = c.create_unicode_buffer(512)
            u.GetWindowTextW(window, name, len(name))
            if u.IsWindowVisible(window) and name.value.startswith("1. "):
                found.append(window)
            return 1

        u.EnumThreadWindows(k.GetCurrentThreadId(), visit, 0)
        if len(found) != 1:
            raise RuntimeError(f"候選窗數量不正確：{len(found)}")
        return found[0]

    def visible_toast(label):
        found = []
        @callback_type
        def visit(window, unused):
            name = c.create_unicode_buffer(512)
            u.GetWindowTextW(window, name, len(name))
            if u.IsWindowVisible(window) and name.value == label:
                found.append(window)
            return 1
        u.EnumThreadWindows(k.GetCurrentThreadId(), visit, 0)
        if len(found) != 1:
            raise RuntimeError(f"狀態提示 {label} 數量不正確：{len(found)}")
        return found[0]

    event(4, 0x10)
    event(5, 0x10)
    event(7, 0x10)
    if ui_less:
        hr, ui_manager = t.query(manager, t.guid("ea1ea135-19df-11d7-a6d2-00065b84435c"))
        t.checked("Query UI manager", hr)
        if not ui_sink.opened:
            raise RuntimeError("中英提示未與宿主協商顯示")
        toast_id = ui_sink.opened[-1]
        toast = t.PTR()
        t.checked("Get mode UI element", t.call(ui_manager, 6, [t.DWORD, t.PPTR], toast_id, c.byref(toast)))
        hr, reading = t.query(toast, t.guid("ea1ea139-19df-11d7-a6d2-00065b84435c"))
        t.checked("Query standard reading UI", hr)
        assert toast_id in ui_sink.updated
        source_context = t.PTR()
        t.checked("Get reading context", t.call(reading, 8, [t.PPTR], c.byref(source_context)))
        assert source_context.value == context.value
        t.release(source_context)
        label = t.PTR()
        t.checked("Read mode text for host drawing", t.call(reading, 9, [t.PPTR], c.byref(label)))
        t.release(reading)
        oleauto = c.WinDLL("oleaut32")
        oleauto.SysStringLen.argtypes = [t.PTR]
        oleauto.SysStringLen.restype = t.ULONG
        oleauto.SysFreeString.argtypes = [t.PTR]
        description = c.wstring_at(label, oleauto.SysStringLen(label))
        oleauto.SysFreeString(label)
        if description != "中":
            raise RuntimeError("宿主收到的中英提示不正確")
        hr, invalid = t.query(toast, t.guid("ea1ea138-19df-11d7-a6d2-00065b84435c"))
        if hr >= 0 or invalid:
            raise RuntimeError("中英提示被錯誤當作候選清單")
        t.release(toast)
        deadline = time.monotonic() + 3
        while toast_id not in ui_sink.closed and time.monotonic() < deadline:
            message = Message()
            while u.PeekMessageW(c.byref(message), None, 0, 0, 1):
                u.DispatchMessageW(c.byref(message))
            time.sleep(0.01)
        if toast_id not in ui_sink.closed:
            raise RuntimeError("中英提示未在限時後結束")
        for expected in ("簡", "繁"):
            previous_opened = len(ui_sink.opened)
            ctrl_slash()
            if len(ui_sink.opened) != previous_opened + 1:
                raise RuntimeError("繁簡提示未與宿主協商顯示")
            toast_id = ui_sink.opened[-1]
            toast = t.PTR()
            t.checked("Get simplification toast", t.call(ui_manager, 6, [t.DWORD, t.PPTR], toast_id, c.byref(toast)))
            label = t.PTR()
            t.checked("Get toast description", t.call(toast, 3, [t.PPTR], c.byref(label)))
            description = c.wstring_at(label, oleauto.SysStringLen(label))
            oleauto.SysFreeString(label)
            t.release(toast)
            if description != expected:
                raise RuntimeError(f"宿主收到的繁簡提示應為 {expected}")
        deadline = time.monotonic() + 3
        while toast_id not in ui_sink.closed and time.monotonic() < deadline:
            message = Message()
            while u.PeekMessageW(c.byref(message), None, 0, 0, 1):
                u.DispatchMessageW(c.byref(message))
            time.sleep(0.01)
        if toast_id not in ui_sink.closed:
            raise RuntimeError("繁簡提示未在限時後結束")
        previous_opened = len(ui_sink.opened)
        previous_updates = len(ui_sink.updated)
        press(0x41)
        if len(ui_sink.opened) != previous_opened + 1:
            raise RuntimeError("無介面模式未通知宿主候選已開啟")
        if ui_sink.opened[-1] not in ui_sink.updated[previous_updates:]:
            raise RuntimeError("無介面模式未通知宿主讀取首次候選內容")
        element = t.PTR()
        t.checked("Get UI element", t.call(ui_manager, 6, [t.DWORD, t.PPTR], ui_sink.opened[-1], c.byref(element)))
        hr, behavior = t.query(element, t.guid("85fad185-58ce-497a-9460-355366b64b9a"))
        t.checked("Query candidate behavior", hr)
        count = t.DWORD()
        t.checked("Get candidate count", t.call(behavior, 9, [c.POINTER(t.DWORD)], c.byref(count)))
        if count.value != 2:
            raise RuntimeError("宿主收到的候選數量不正確")
        for expected in ("簡", "繁"):
            ctrl_slash()
            toast = t.PTR()
            t.checked("Get composing toast", t.call(ui_manager, 6, [t.DWORD, t.PPTR], ui_sink.opened[-1], c.byref(toast)))
            label = t.PTR()
            t.checked("Get composing toast description", t.call(toast, 3, [t.PPTR], c.byref(label)))
            description = c.wstring_at(label, oleauto.SysStringLen(label))
            oleauto.SysFreeString(label)
            t.release(toast)
            if description != expected or store.content() != "日":
                raise RuntimeError("組字中的繁簡提示或文字不正確")
        document_from_element = t.PTR()
        t.checked("Candidate document", t.call(behavior, 8, [t.PPTR], c.byref(document_from_element)))
        t.release(document_from_element)
        t.checked("Select second candidate", t.call(behavior, 15, [t.DWORD], 1))
        t.checked("Finalize candidate", t.call(behavior, 16, []))
        t.release(behavior)
        t.release(element)
        if store.content() != "曰" or not ui_sink.closed:
            raise RuntimeError("無介面模式未提交或結束宿主候選")
        press(0x41)
        element = t.PTR()
        t.checked("Get next UI element", t.call(ui_manager, 6, [t.DWORD, t.PPTR], ui_sink.opened[-1], c.byref(element)))
        hr, behavior = t.query(element, t.guid("85fad185-58ce-497a-9460-355366b64b9a"))
        t.checked("Query next behavior", hr)
        t.checked("Abort composition", t.call(behavior, 17, []))
        t.release(behavior)
        t.release(element)
        if store.content() != "曰":
            raise RuntimeError("宿主取消候選後未清除字根")
        previous_opened = len(ui_sink.opened)
        previous_updates = len(ui_sink.updated)
        press(0x73)
        if len(ui_sink.opened) != previous_opened + 1:
            raise RuntimeError("無介面模式未通知宿主 F4 設定已開啟")
        if ui_sink.opened[-1] not in ui_sink.updated[previous_updates:]:
            raise RuntimeError("無介面模式未通知宿主讀取首次 F4 設定內容")
        element = t.PTR()
        t.checked("Get settings element", t.call(ui_manager, 6, [t.DWORD, t.PPTR], ui_sink.opened[-1], c.byref(element)))
        hr, behavior = t.query(element, t.guid("85fad185-58ce-497a-9460-355366b64b9a"))
        t.checked("Query settings behavior", hr)
        t.checked("Get setting count", t.call(behavior, 9, [c.POINTER(t.DWORD)], c.byref(count)))
        if count.value != 5:
            raise RuntimeError("宿主未收到五項 F4 設定")
        oleauto = c.WinDLL("oleaut32")
        oleauto.SysStringLen.argtypes = [t.PTR]
        oleauto.SysStringLen.restype = t.ULONG
        oleauto.SysFreeString.argtypes = [t.PTR]
        for index, name in enumerate(("字集", "繁簡", "預設輸入模式", "字形", "標點")):
            value = t.PTR()
            t.checked("Get setting", t.call(behavior, 11, [t.DWORD, t.PPTR], index, c.byref(value)))
            text = c.wstring_at(value, oleauto.SysStringLen(value))
            oleauto.SysFreeString(value)
            if not text.startswith(name + "　"):
                raise RuntimeError("F4 設定項目或次序不正確")
        t.checked("Select setting", t.call(behavior, 15, [t.DWORD], 1))
        t.checked("Toggle setting", t.call(behavior, 16, []))
        document_from_element = t.PTR()
        t.checked("Settings document after toggle", t.call(behavior, 8, [t.PPTR], c.byref(document_from_element)))
        t.release(document_from_element)
        t.release(behavior)
        t.release(element)
        press(0x73)
        t.release(ui_manager)
        print("PASS: 真實 TSF UIElement 管理員與無介面宿主完成選字及取消。", flush=True)
    else:
        reference = Rect()
        if not u.GetWindowRect(visible_toast("中"), c.byref(reference)):
            raise OSError(c.get_last_error(), "無法取得中英提示位置")
        for label in ("簡", "繁"):
            ctrl_slash()
            toast = visible_toast(label)
            actual = Rect()
            if not u.GetWindowRect(toast, c.byref(actual)):
                raise OSError(c.get_last_error(), "無法取得繁簡提示位置")
            if (actual.left, actual.top, actual.right, actual.bottom) != (reference.left, reference.top, reference.right, reference.bottom):
                raise RuntimeError("繁簡提示位置或大小與中英提示不一致")
        press(0xC0)
        visible_toast("粵拼模式")
        press(0xC0)
        visible_toast("倉頡模式")
        if store.content():
            raise RuntimeError("取消空白粵拼模式不應上屏")
        press(0xC0)
        visible_toast("粵拼模式")
        for key in "NGO":
            press(ord(key))
        if store.content() != "ngo":
            raise RuntimeError("粵拼碼未顯示在組字中")
        candidate_window()
        press(0xC0)
        visible_toast("倉頡模式")
        if store.content():
            raise RuntimeError("切回倉頡時應清除未完成的粵拼")
        print("PASS: 反引號可取消空白或正在組字的粵拼模式，切換時顯示正確提示。", flush=True)
        press(0x41)
        press(0xC0)
        if store.content() != "日":
            raise RuntimeError("組字中的反引號不應切換倉頡或輸入標點")
        for label in ("簡", "繁"):
            ctrl_slash()
            toast = visible_toast(label)
            if store.content() != "日":
                raise RuntimeError("組字中切換繁簡時，字碼不應提交或消失")
            candidate_rect, toast_rect = Rect(), Rect()
            assert u.GetWindowRect(candidate_window(), c.byref(candidate_rect))
            assert u.GetWindowRect(toast, c.byref(toast_rect))
            assert (
                toast_rect.right <= candidate_rect.left or toast_rect.left >= candidate_rect.right
                or toast_rect.bottom <= candidate_rect.top or toast_rect.top >= candidate_rect.bottom
            ), "狀態提示與候選窗重疊"
        before = store.write_count
        click(candidate_window(), 16, 16)
        if store.content() != "日" or store.write_count - before != 1:
            raise RuntimeError("左鍵點擊候選時未提交一次完整文字")
        press(0x41)
        click(hwnd, 5, 5)
        if store.content() != "日":
            raise RuntimeError("點擊候選窗外時未取消未完成字根")
        for _ in range(3):
            press(0x5a)
        before = store.write_count
        click(candidate_window(), 16, 16)
        expected = "長" * 30 + "\n第二行"
        if store.content() != "日" + expected or not store.wrote_payload_once(before, expected):
            raise RuntimeError("滑鼠選取長文時未以一次寫入完整上屏")
        print("PASS: 滑鼠點擊候選與框外取消；長文多行一次完整寫入。", flush=True)

        before = store.write_count
        press(0x73)  # F4
        menu = candidate_window()
        label = c.create_unicode_buffer(512)
        u.GetWindowTextW(menu, label, len(label))
        original_label = label.value
        click(menu, 16, 16)
        u.GetWindowTextW(menu, label, len(label))
        if not u.IsWindowVisible(menu) or label.value == original_label:
            raise RuntimeError("點擊 F4 選單內的選項應切換設定並保留選單")
        click(menu, 16, 16)  # 還原字集選項
        click(hwnd, 5, 5)
        if u.IsWindowVisible(menu) or store.write_count != before:
            raise RuntimeError("點擊 F4 選單外應關閉選單且不寫入文字")
        press(0x73)
        menu = candidate_window()
        t.checked("F4 focus loss", t.call(sink, 3, [t.LONG], 0))
        if u.IsWindowVisible(menu):
            raise RuntimeError("F4 選單失去焦點後仍顯示")
        t.checked("Return to the test control", t.call(sink, 3, [t.LONG], 1))
        print("PASS: F4 點擊內部切換，點擊外部或失去焦點即關閉。", flush=True)

        def enter_jyutping():
            press(0xC0)
            visible_toast("粵拼模式")
            for key in "NGO":
                press(ord(key))

        for key, suffix in ((0x20, "我"), (0x31, "我"), (0x0D, "ngo"), (0x1B, "")):
            before = store.content()
            enter_jyutping()
            press(key)
            visible_toast("倉頡模式")
            if store.content() != before + suffix:
                raise RuntimeError(f"粵拼返回倉頡的文字結果不正確：{key:#x}")
        before = store.content()
        enter_jyutping()
        click(candidate_window(), 16, 16)
        visible_toast("倉頡模式")
        if store.content() != before + "我":
            raise RuntimeError("滑鼠選取粵拼候選後未正確上屏")
        before = store.content()
        enter_jyutping()
        click(hwnd, 5, 5)
        visible_toast("倉頡模式")
        if store.content() != before:
            raise RuntimeError("點擊候選框外時未取消粵拼")
        print("PASS: 粵拼用鍵盤或滑鼠選字、提交原碼或取消時，返回倉頡會顯示提示。", flush=True)

        before = store.content()
        store.defer_write_once = True
        press(0x41)
        assert store.deferred_lock and store.content() == before, "宿主應先延後編輯鎖"
        result = t.HR()
        flags, store.deferred_lock = store.deferred_lock, 0
        t.checked("Grant deferred edit lock", store.request_lock(store.ptr, flags, c.pointer(result)))
        t.checked("Deferred edit should run", result.value)
        deadline = time.monotonic() + 2
        while store.content() == before and time.monotonic() < deadline:
            message = Message()
            while u.PeekMessageW(c.byref(message), None, 0, 0, 1):
                u.DispatchMessageW(c.byref(message))
            time.sleep(0.01)
        assert not store.defer_write_once, "未重現宿主拒絕同步編輯鎖"
        assert store.content() == before + "日", "非同步重試應只寫入一次"
        candidate_window()
        press(0x1B)
        assert store.content() == before
        print("PASS: 拒絕第一次同步鎖後，TSF 重試一次並恢復候選。", flush=True)

    t.release(sink)
    t.checked("Deactivate text service", t.call(processor, 4, []))
    if source:
        t.checked("Unadvise UI sink", t.call(source, 4, [t.DWORD], sink_cookie.value))
        t.release(source)
    t.checked("Clear focus", t.call(manager, 8, [t.PTR], None))
    t.checked("Pop", t.call(document, 5, [t.DWORD], 1))
    u.DestroyWindow(hwnd)
    t.release(context)
    t.release(document)
    t.checked("Deactivate", t.call(manager, 4, []))
    t.release(processor)
    t.release(manager)
    t.release(factory)
    ole.CoUninitialize()
    dll.DllCanUnloadNow.restype = t.HR
    if dll.DllCanUnloadNow() != 0:
        raise RuntimeError("測試後仍有未釋放的輸入法物件")
    if not k.FreeLibrary(dll._handle):
        raise OSError(c.get_last_error(), "無法釋放測試 DLL")
    u.CloseDesktop(desktop)


def run_cases(dll):
    failures = []

    def run(ui_less):
        try:
            worker(dll, ui_less)
        except BaseException as exc:
            failures.append(exc)

    for ui_less in (False, True):
        thread = threading.Thread(target=run, args=(ui_less,))
        thread.start()
        thread.join(30)
        if thread.is_alive():
            raise RuntimeError("隔離桌面測試逾時")
        if failures:
            raise failures[0]


def main():
    source = Path(sys.argv[1]).resolve()
    if "--worker" in sys.argv:
        run_cases(source)
        return
    work = Path(__file__).resolve().parent.parent / "work"
    work.mkdir(exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="tsf-mouse-", dir=work) as name:
        root = Path(name)
        dictionaries = root / "data" / "dictionaries"
        dictionaries.mkdir(parents=True)
        (root / "outputs").mkdir()
        (dictionaries / "cangjie3.dict.yaml").write_text("---\n...\n日\ta\n曰\ta\n", encoding="utf-8")
        (dictionaries / "custom.dict.yaml").write_text("---\n...\n" + "長" * 30 + "\\n第二行\tzzz\n", encoding="utf-8")
        # 固定第一候選，將公用字庫的排序驗證留給實際字庫測試。
        (dictionaries / "jyut6ping3.chars.dict.csv.json").write_text('{"我":{"ngo5":"預設"}}', encoding="utf-8")
        dll = root / "outputs" / "rust_ime_fixture.dll"
        shutil.copy2(source, dll)
        # 讓宿主程序先結束，Windows 便能釋放測試 DLL，再由父程序清理目錄。
        subprocess.run([sys.executable, str(Path(__file__).resolve()), str(dll), "--worker"],
                       check=True, timeout=75)


if __name__ == "__main__":
    main()
