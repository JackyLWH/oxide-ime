"""在真正的 AppContainer token 下測試 TSF；所有字典內容均為合成測試資料。"""
import ctypes as c
from ctypes import wintypes as w
from pathlib import Path
import struct
import sys

sys.dont_write_bytecode = True
import tsf_real_store_test as t


def check(label, hr):
    if hr < 0:
        raise RuntimeError(f"{label}: {t.hr_text(hr)}")
    return hr


def main():
    dll_path, private_path, pipe_name = sys.argv[1:4]
    kernel = c.WinDLL("kernel32", use_last_error=True)
    advapi = c.WinDLL("advapi32", use_last_error=True)
    kernel.GetCurrentProcess.restype = t.PTR
    kernel.CloseHandle.argtypes = [t.PTR]
    kernel.CreateFileW.argtypes = [w.LPCWSTR, w.DWORD, w.DWORD, t.PTR, w.DWORD, w.DWORD, t.PTR]
    kernel.CreateFileW.restype = t.PTR
    kernel.CreateNamedPipeW.argtypes = [w.LPCWSTR, w.DWORD, w.DWORD, w.DWORD, w.DWORD, w.DWORD, w.DWORD, t.PTR]
    kernel.CreateNamedPipeW.restype = t.PTR
    kernel.OpenEventW.argtypes = [w.DWORD, w.BOOL, w.LPCWSTR]
    kernel.OpenEventW.restype = t.PTR
    kernel.GetModuleHandleW.argtypes = [w.LPCWSTR]
    kernel.GetModuleHandleW.restype = t.PTR
    kernel.ReadFile.argtypes = [t.PTR, t.PTR, w.DWORD, c.POINTER(w.DWORD), t.PTR]
    kernel.WriteFile.argtypes = [t.PTR, t.PTR, w.DWORD, c.POINTER(w.DWORD), t.PTR]
    advapi.OpenProcessToken.argtypes = [t.PTR, w.DWORD, t.PPTR]
    advapi.GetTokenInformation.argtypes = [t.PTR, w.DWORD, t.PTR, w.DWORD, c.POINTER(w.DWORD)]
    token = t.PTR()
    assert advapi.OpenProcessToken(kernel.GetCurrentProcess(), 8, c.byref(token))
    try:
        inside, used = w.DWORD(), w.DWORD()
        assert advapi.GetTokenInformation(token, 29, c.byref(inside), 4, c.byref(used)) and inside.value == 1
    finally:
        kernel.CloseHandle(token)
    invalid = t.PTR(-1).value
    ordinary_only = kernel.CreateFileW(sys.argv[5], 0x80000000, 7, None, 3, 0, None)
    if sys.argv[4] == "lpac":
        if ordinary_only != invalid:
            kernel.CloseHandle(ordinary_only)
            raise RuntimeError("LPAC 不應讀到只開放一般 AppContainer 的測試檔案")
        if c.get_last_error() != 5:
            raise RuntimeError("LPAC 隔離測試未取得拒絕存取")
    else:
        if ordinary_only == invalid:
            raise RuntimeError(f"一般 AppContainer 應可讀取測試檔案：{c.get_last_error()}")
        kernel.CloseHandle(ordinary_only)
    direct = kernel.CreateFileW(private_path, 0x80000000, 7, None, 3, 0, None)
    if direct != invalid:
        kernel.CloseHandle(direct)
        raise RuntimeError("AppContainer 不應直接讀取私人測試字典")
    if c.get_last_error() != 5:
        raise RuntimeError("私人測試字典沒有以拒絕存取的方式隔離")
    spoof = kernel.CreateNamedPipeW(pipe_name, 0x40000003, 8, 12, 4096, 4096, 0, None)
    if spoof != invalid:
        kernel.CloseHandle(spoof)
        raise RuntimeError("AppContainer 不應能夠建立字庫服務的管道實例")
    if c.get_last_error() != 5:
        raise RuntimeError("管道建立權限測試沒有取得預期的拒絕存取")
    stop_name = "Local\\" + pipe_name.rsplit("\\", 1)[-1] + "-stop"
    stop = kernel.OpenEventW(2, False, stop_name)  # EVENT_MODIFY_STATE
    if stop:
        kernel.CloseHandle(stop)
        raise RuntimeError("AppContainer 不應能夠通知字庫服務停止")
    if c.get_last_error() not in (2, 5):
        raise RuntimeError("字庫服務停止事件沒有以命名空間或存取權限隔離")

    def convert(text, allowed):
        payload = text.encode("utf-8")
        short_length = len(payload) if len(payload) <= 0xffff else 0
        request = struct.pack("<4sBBH", b"RI01", 0, 5, short_length)
        if not short_length:
            request += struct.pack("<I", len(payload))
        try:
            pipe = kernel.CreateFileW(pipe_name, 0x00120003, 0, None, 3, 0, None)
            if pipe == invalid:
                raise c.WinError(c.get_last_error())
            try:
                request += payload
                offset = 0
                while offset < len(request):
                    chunk = c.create_string_buffer(request[offset:])
                    written = w.DWORD()
                    if not kernel.WriteFile(pipe, chunk, len(request) - offset, c.byref(written), None) or not written.value:
                        raise c.WinError(c.get_last_error())
                    offset += written.value
                def read_exact(length):
                    result = bytearray()
                    while len(result) < length:
                        part, received = c.create_string_buffer(length - len(result)), w.DWORD()
                        if not kernel.ReadFile(pipe, part, len(part), c.byref(received), None) or not received.value:
                            break
                        result.extend(part.raw[:received.value])
                    return bytes(result)
                header = read_exact(16)
                if len(header) != 16:
                    if allowed:
                        raise RuntimeError("已有剪貼簿轉換權限時，未收到轉換結果")
                    return
                magic, status, count, length = struct.unpack("<4sIII", header)
                body = read_exact(length)
                if not allowed:
                    raise RuntimeError("沒有剪貼簿轉換權限時，不應套用私人轉換規則")
                if magic != b"RI01" or status or count != 1 or len(body) != length:
                    raise RuntimeError("私人轉換回應無效")
                text_length = struct.unpack_from("<I", body)[0]
                result = body[4:].decode("utf-8")
                if text_length != len(body) - 4 or result != text.replace("简体", "繁體"):
                    raise RuntimeError("合成轉換結果不正確")
            finally:
                kernel.CloseHandle(pipe)
        except OSError:
            if allowed:
                raise

    convert("简体", False)
    print("READY_INITIAL_GRANT", flush=True)
    assert sys.stdin.readline().strip() == "other allowed"
    convert("简体", False)

    user32 = c.WinDLL("user32", use_last_error=True)
    user32.CreateWindowExW.argtypes = [w.DWORD, w.LPCWSTR, w.LPCWSTR, w.DWORD, c.c_int, c.c_int, c.c_int, c.c_int, t.PTR, t.PTR, t.PTR, t.PTR]
    user32.CreateWindowExW.restype = t.PTR
    user32.DestroyWindow.argtypes = [t.PTR]
    candidate_window = user32.CreateWindowExW(0x08080088, "STATIC", "", 0x80000000, 0, 0, 80, 30, None, None, None, None)
    if candidate_window:
        user32.DestroyWindow(candidate_window)
        print("沙盒可建立隱藏的候選視窗。", flush=True)
    else:
        print(f"沙盒建立候選視窗失敗：{c.get_last_error()}", flush=True)

    pipe = kernel.CreateFileW(pipe_name, 0x00120003, 0, None, 3, 0, None)
    if pipe == invalid:
        raise RuntimeError(f"沙盒無法連接測試字庫管道：{c.get_last_error()}")
    try:
        request = c.create_string_buffer(b"RI01\x01\x00\x01\x00z")
        written = w.DWORD()
        if not kernel.WriteFile(pipe, request, 9, c.byref(written), None) or written.value != 9:
            raise RuntimeError("測試查詢傳送失敗")
        def read_exact(length):
            result = bytearray()
            while len(result) < length:
                data, count = c.create_string_buffer(length - len(result)), w.DWORD()
                if not kernel.ReadFile(pipe, data, len(data), c.byref(count), None) or not count.value:
                    raise RuntimeError("測試查詢回應不完整")
                result.extend(data.raw[:count.value])
            return bytes(result)
        magic, status, count, length = struct.unpack("<4sIII", read_exact(16))
        body = read_exact(length)
        if magic != b"RI01" or status != 0 or count != 1 or "第二行".encode() in body:
            raise RuntimeError("查字回應應只包含縮略文字")
        snapshot, text_length = struct.unpack_from("<QI", body)
        text = body[12:].decode("utf-8")
        if not snapshot or text_length != len(body) - 12 or not text.endswith("…") or len(text) != 21:
            raise RuntimeError("長詞條縮略文字或快照不正確")
    finally:
        kernel.CloseHandle(pipe)
    # 長詞條超過一般反查請求的長度上限，透過本程序快照仍可取得完整字碼。
    pipe = kernel.CreateFileW(pipe_name, 0x00120003, 0, None, 3, 0, None)
    if pipe == invalid:
        raise c.WinError(c.get_last_error())
    try:
        request = c.create_string_buffer(struct.pack("<4sBBHQI", b"RI01", 0, 6, 12, snapshot, 0))
        written = w.DWORD()
        if not kernel.WriteFile(pipe, request, 20, c.byref(written), None) or written.value != 20:
            raise RuntimeError("反查請求傳送失敗")
        magic, status, count, length = struct.unpack("<4sIII", read_exact(16))
        body = read_exact(length)
        if magic != b"RI01" or status or count != 1 or body != struct.pack("<I", 1) + b"z":
            raise RuntimeError("以候選快照反查長詞條字碼失敗")
    finally:
        kernel.CloseHandle(pipe)
    print(f"SNAPSHOT_TEST {snapshot}", flush=True)
    if sys.stdin.readline().strip() != "snapshot checked":
        raise RuntimeError("測試未確認候選快照隔離")

    ole = c.WinDLL("ole32")
    ole.CoInitializeEx.argtypes = [t.PTR, t.DWORD]
    ole.CoInitializeEx.restype = t.HR
    ole.CoCreateInstance.argtypes = [c.POINTER(t.Guid), t.PTR, t.DWORD, c.POINTER(t.Guid), t.PPTR]
    ole.CoCreateInstance.restype = t.HR
    ole.CoRegisterClassObject.argtypes = [c.POINTER(t.Guid), t.PTR, t.DWORD, t.DWORD, c.POINTER(t.DWORD)]
    ole.CoRegisterClassObject.restype = t.HR
    check("CoInitializeEx", ole.CoInitializeEx(None, 2))
    dll = c.WinDLL(str(Path(dll_path)))
    dll.DllGetClassObject.argtypes = [c.POINTER(t.Guid), c.POINTER(t.Guid), t.PPTR]
    dll.DllGetClassObject.restype = t.HR
    factory = t.PTR()
    check("factory", dll.DllGetClassObject(c.byref(t.OUR_CLSID), c.byref(t.ICLASSFACTORY), c.byref(factory)))
    proxy = t.FactoryProxy(factory)
    registration = t.DWORD()
    check("register test factory", ole.CoRegisterClassObject(c.byref(t.OUR_CLSID), proxy.ptr, 1, 1, c.byref(registration)))
    manager = t.create_com(ole, "529a9e6b-6587-4f23-ab9e-9c7d683e3c50", "3e90ade3-7594-4cb0-bb58-69628f5f458c")
    profiles = t.create_com(ole, "33c53a50-f456-4884-b049-85fd643ecfed", "71c6e74c-0f28-11d8-a82a-00065b84435c")
    client = t.DWORD()
    check("ActivateEx UIElement only", t.call(manager, 14, [c.POINTER(t.DWORD), t.DWORD], c.byref(client), 4))
    hr, ui_source = t.query(manager, t.guid("4ea48a35-60ae-446f-8fd6-e6a8d82459f7"))
    check("query UI source", hr)
    hr, ui_manager = t.query(manager, t.guid("ea1ea135-19df-11d7-a6d2-00065b84435c"))
    check("query UI manager", hr)
    ui_sink = t.UiElementSink()
    ui_sink_id = t.guid("ea1ea136-19df-11d7-a6d2-00065b84435c")
    ui_cookie = t.DWORD()
    check("advise UI sink", t.call(ui_source, 3, [c.POINTER(t.Guid), t.PTR, c.POINTER(t.DWORD)], c.byref(ui_sink_id), ui_sink.ptr, c.byref(ui_cookie)))
    document = t.PTR()
    check("document", t.call(manager, 5, [t.PPTR], c.byref(document)))
    store = t.MemoryTextStore()
    context, cookie = t.PTR(), t.DWORD()
    check("context", t.call(document, 3, [t.DWORD, t.DWORD, t.PTR, t.PPTR, c.POINTER(t.DWORD)], client.value, 0, store.ptr, c.byref(context), c.byref(cookie)))
    check("push", t.call(document, 4, [t.PTR], context))
    check("focus", t.call(manager, 8, [t.PTR], document))
    check("activate profile", t.call(profiles, 3, [t.DWORD, c.c_uint16, c.POINTER(t.Guid), c.POINTER(t.Guid), t.PTR, t.DWORD], 1, 0x0404, c.byref(t.OUR_CLSID), c.byref(t.PROFILE), None, 0x10000004))
    if not proxy.processor:
        raise RuntimeError("AppContainer 內沒有啟用文字服務")
    hr, sink = t.query(proxy.processor, t.IKEYEVENTSINK)
    check("key sink", hr)

    def event(slot, value):
        eaten = t.LONG()
        check("key", t.call(sink, slot, [t.PTR, c.c_size_t, c.c_ssize_t, c.POINTER(t.LONG)], context, value, 0, c.byref(eaten)))
        return eaten.value

    def press(value):
        if event(4, value) != 1 or event(6, value) != 1:
            raise RuntimeError(f"文字服務沒有處理按鍵 {value:#x}")
        if event(5, value):
            event(7, value)

    event(4, 0x10)
    if event(5, 0x10) != 1 or event(7, 0x10) != 0:
        raise RuntimeError("Shift 未能切換中文")
    previous_opened, previous_updates = len(ui_sink.opened), len(ui_sink.updated)
    press(ord("A"))
    if store.content() != "日":
        raise RuntimeError("倉頡組字文字不正確")
    opened = ui_sink.opened[previous_opened:]
    if len(opened) != 1 or opened[0] not in ui_sink.updated[previous_updates:]:
        raise RuntimeError("首次沙盒組字應只顯示候選")
    oleauto = c.WinDLL("oleaut32")
    oleauto.SysStringLen.argtypes = [t.PTR]
    oleauto.SysStringLen.restype = t.ULONG
    oleauto.SysFreeString.argtypes = [t.PTR]
    candidate_id = opened[0]
    element = t.PTR()
    check("host UI element", t.call(ui_manager, 6, [t.DWORD, t.PPTR], candidate_id, c.byref(element)))
    hr, candidates = t.query(element, t.guid("85fad185-58ce-497a-9460-355366b64b9a"))
    check("sandbox candidate UI", hr)
    t.release(element)
    count = t.DWORD()
    check("candidate count", t.call(candidates, 9, [c.POINTER(t.DWORD)], c.byref(count)))
    if count.value < 2:
        raise RuntimeError("沙盒宿主收到的候選數量不正確")
    value = t.PTR()
    check("first candidate", t.call(candidates, 11, [t.DWORD, t.PPTR], 0, c.byref(value)))
    first = c.wstring_at(value, oleauto.SysStringLen(value))
    oleauto.SysFreeString(value)
    t.release(candidates)
    if first != "日":
        raise RuntimeError("沙盒宿主收到的首個候選不正確")
    press(ord("2"))
    if store.content() != "曰":
        raise RuntimeError(f"第二候選未能上屏，測試文字：{store.content()!r}")
    if candidate_id not in ui_sink.closed:
        raise RuntimeError("選字後沙盒宿主未收到候選結束通知")

    press(0xC0)
    for letter in "NGOXX":
        press(ord(letter))
    if not store.content().endswith("ngoxx"):
        raise RuntimeError("粵拼反查組字應顯示原始拼音")
    press(ord("1"))
    if not store.content().endswith("我"):
        raise RuntimeError("粵拼反查未能選出第五聲的「我」")
    # 以下既有測試使用固定的文字前綴。
    press(0xC0)
    press(0x1B)
    before_homophone = store.content()
    for letter in "AZX":
        press(ord(letter))
    press(ord("2"))
    if store.content() != before_homophone + "逸":
        raise RuntimeError("沙盒內同音完整音搜尋或選字失敗")
    before_reload = store.content()

    # 詞條更改前已顯示的候選仍須完整提交；下次組字才採用新版。
    press(ord("Z"))
    print("READY_FOR_RELOAD", flush=True)
    if sys.stdin.readline().strip() != "continue":
        raise RuntimeError("測試未收到重載訊號")
    writes = store.write_count
    press(ord("1"))
    old = "這是一條長文測試詞條超過二十個字元測試完整輸出" + "長" * 100 + "\n第二行"
    if store.content() != before_reload + old or not store.wrote_payload_once(writes, old):
        raise RuntimeError("重載前的多行長詞條未以一次編輯完整上屏")
    press(ord("Z"))
    press(ord("1"))
    if not store.content().endswith("即時更新詞條"):
        raise RuntimeError("沙盒內的既有程序未採用更新後字典")

    previous_opened, previous_updates = len(ui_sink.opened), len(ui_sink.updated)
    press(0x73)
    if len(ui_sink.opened) != previous_opened + 1 or ui_sink.opened[-1] not in ui_sink.updated[previous_updates:]:
        raise RuntimeError("沙盒宿主沒有收到首次 F4 選單顯示和內容更新")
    press(ord("3"))  # 設定下次登入預設中文，當前組字行為維持。
    press(0x73)
    keyboard = (c.c_byte * 256)()
    if not user32.GetKeyboardState(c.byref(keyboard)):
        raise c.WinError(c.get_last_error())
    saved = bytes(keyboard)
    keyboard[0x11] = -128
    if not user32.SetKeyboardState(c.byref(keyboard)):
        raise c.WinError(c.get_last_error())
    try:
        press(0xBF)  # Ctrl+/ 將整個登入工作階段切為簡體。
    finally:
        user32.SetKeyboardState(c.byref((c.c_byte * 256).from_buffer_copy(saved)))
    press(ord("S"))
    press(ord("1"))
    if not store.content().endswith("发"):
        raise RuntimeError("沙盒內繁轉簡失敗")
    press(0xC0)  # 粵拼尚未輸入，另一個程序應可繼承。
    print("READY_SHARED", flush=True)
    if sys.stdin.readline().strip() != "shared":
        raise RuntimeError("測試未收到桌面宿主切換中英模式的訊號。")
    hr, language_bar = t.query(manager, t.guid("ba468c55-9956-4fb1-a59d-52a7dd7cc6aa"))
    check("language bar manager", hr)
    item, label = t.PTR(), t.PTR()
    mode_id = t.guid("2c77a81e-41cc-4178-a3a7-5f8a987568e6")
    check("mode indicator item", t.call(language_bar, 4, [c.POINTER(t.Guid), t.PPTR], c.byref(mode_id), c.byref(item)))
    check("mode indicator after peer change", t.call(item, 11, [t.PPTR], c.byref(label)))
    badge = c.wstring_at(label, oleauto.SysStringLen(label))
    oleauto.SysFreeString(label)
    t.release(item)
    t.release(language_bar)
    if badge != "英" or event(4, ord("A")) != 0:
        raise RuntimeError("沙盒沒有繼承桌面宿主切換的英文模式。")
    print("PASS: 中英指示器在本程序再次按鍵前已採用其他程序的模式。", flush=True)
    event(4, 0x10)
    event(7, 0x10)
    press(0x73)
    press(ord("1"))  # 香港字 → 全字
    press(0x73)
    press(ord("S"))
    press(ord("X"))
    press(ord("1"))
    if not store.content().endswith("𠀀"):
        raise RuntimeError("沙盒內全字選項或補充平面字元失敗")

    press(ord("Z"))
    before_disconnect = store.content()
    print("READY_OFFLINE", flush=True)
    if sys.stdin.readline().strip() != "offline":
        raise RuntimeError("測試未收到離線訊號")
    press(ord("1"))
    if store.content() != before_disconnect:
        raise RuntimeError("字庫中斷時不應提交錯誤文字或清除組字")
    press(0x08)
    if store.content() != before_disconnect[:-1]:
        raise RuntimeError("字庫中斷後無法取消組字")
    print("READY_RESTART", flush=True)
    if sys.stdin.readline().strip() != "restarted":
        raise RuntimeError("測試未收到重啟訊號")
    convert("简体", False)
    press(ord("Z"))
    press(ord("1"))
    if not store.content().endswith("即时更新词条"):
        raise RuntimeError("重啟後私人詞條應可直接選取，亦應保留簡體模式")

    def check_menu():
        press(0x73)
        element = t.PTR()
        check("F4 UI element", t.call(ui_manager, 6, [t.DWORD, t.PPTR], ui_sink.opened[-1], c.byref(element)))
        hr, candidates = t.query(element, t.guid("85fad185-58ce-497a-9460-355366b64b9a"))
        check("F4 menu list", hr)
        count = t.DWORD()
        check("F4 option count", t.call(candidates, 9, [c.POINTER(t.DWORD)], c.byref(count)))
        if count.value != 6:
            raise RuntimeError("沙盒 F4 應與一般宿主顯示同樣六項設定")
        value = t.PTR()
        check("F4 startup state", t.call(candidates, 11, [t.DWORD, t.PPTR], 2, c.byref(value)))
        actual = c.wstring_at(value, oleauto.SysStringLen(value))
        oleauto.SysFreeString(value)
        t.release(candidates)
        t.release(element)
        if actual != "預設輸入模式　中文":
            raise RuntimeError("字庫服務重啟後遺失 F4 保存的預設輸入模式")
        press(0x1B)

    check_menu()
    before_clipboard = store.content()
    press(ord("Z"))
    press(ord("V"))
    raw_preedit = store.content()
    press(ord("2"))
    if store.content() != raw_preedit:
        raise RuntimeError("字庫開放後，仍須檢查私人剪貼簿轉換權限")
    press(ord("1"))
    if store.content() != before_clipboard + "合成简体\n第二行🙂":
        raise RuntimeError("沙盒合成剪貼簿原文未完整上屏")
    print("READY_CLIPBOARD", flush=True)
    if sys.stdin.readline().strip() != "clipboard":
        raise RuntimeError("測試未收到剪貼簿轉換啟用訊號")
    # 請求包含超過 64 KiB 的合成內容，驗證完整回應和延長封包。
    convert("简体" * 12000, True)
    check_menu()
    press(0x73)
    press(ord("2"))  # 簡 → 繁
    press(0x73)
    press(ord("Z"))
    press(ord("1"))
    if not store.content().endswith("即時更新詞條"):
        raise RuntimeError("字庫恢復後無法重新查字")

    before_clipboard = store.content()
    press(ord("Z"))
    press(ord("V"))
    press(ord("2"))
    if store.content() != before_clipboard + "合成繁體\n第二行🙂":
        raise RuntimeError("有剪貼簿權限時，沙盒簡轉繁候選未完整上屏")

    before = store.content()
    press(0x73)
    press(ord("6"))  # 倉頡 → 速成
    press(0x73)
    press(ord("O"))
    press(ord("F"))
    press(ord("1"))
    assert store.content() == before + "你", "沙盒速成應經字庫服務查詢並上屏"
    before = store.content()
    press(0x73)
    press(ord("6"))  # 速成 → 筆劃
    press(0x73)
    press(ord("L"))
    element = t.PTR()
    check("Stroke UI element", t.call(ui_manager, 6, [t.DWORD, t.PPTR], ui_sink.opened[-1], c.byref(element)))
    hr, candidates = t.query(element, t.guid("ea1ea138-19df-11d7-a6d2-00065b84435c"))
    check("Stroke candidates", hr)
    count = t.DWORD()
    check("Full wildcard count", t.call(candidates, 9, [c.POINTER(t.DWORD)], c.byref(count)))
    assert count.value > 100_000, f"通配首筆應涵蓋整份碼表，實際收到 {count.value} 個候選"
    t.release(candidates)
    t.release(element)
    press(0x1B)
    press(0x68)
    press(ord("K"))
    press(0x67)
    press(0x67)
    press(ord("1"))
    assert store.content() == before + "日", "沙盒筆劃應經字庫服務查詢並上屏"
    print("PASS: 沙盒速成、全字筆劃通配、數字小鍵盤選碼及完整上屏。", flush=True)

    t.release(sink)
    check("unfocus", t.call(manager, 8, [t.PTR], None))
    check("pop", t.call(document, 5, [t.DWORD], 1))
    t.release(context)
    t.release(document)
    check("unadvise UI sink", t.call(ui_source, 4, [t.DWORD], ui_cookie.value))
    t.release(ui_source)
    t.release(ui_manager)
    check("deactivate", t.call(manager, 4, []))
    t.release(proxy.processor)
    t.release(manager)
    t.release(profiles)
    check("revoke", ole.CoRevokeClassObject(registration.value))
    t.release(factory)
    if len(sys.argv) > 6:
        registered = t.PTR()
        check("系統登錄的文字服務", ole.CoCreateInstance(c.byref(t.OUR_CLSID), None, 1, c.byref(t.ITEXTINPUTPROCESSOR), c.byref(registered)))
        if not kernel.GetModuleHandleW(Path(sys.argv[6]).name):
            raise RuntimeError("沙盒未載入目前登錄的輸入法 DLL")
        t.release(registered)
        ole.CoGetClassObject.argtypes = [c.POINTER(t.Guid), t.DWORD, t.PTR, c.POINTER(t.Guid), t.PPTR]
        ole.CoGetClassObject.restype = t.HR
        system_factory = t.PTR()
        check("取得已登錄類別工廠", ole.CoGetClassObject(c.byref(t.OUR_CLSID), 1, None, c.byref(t.ICLASSFACTORY), c.byref(system_factory)))
        system_proxy, registration = t.FactoryProxy(system_factory), t.DWORD()
        check("建立正式版測試代理", ole.CoRegisterClassObject(c.byref(t.OUR_CLSID), system_proxy.ptr, 1, 1, c.byref(registration)))
        manager = t.create_com(ole, "529a9e6b-6587-4f23-ab9e-9c7d683e3c50", "3e90ade3-7594-4cb0-bb58-69628f5f458c")
        profiles = t.create_com(ole, "33c53a50-f456-4884-b049-85fd643ecfed", "71c6e74c-0f28-11d8-a82a-00065b84435c")
        client = t.DWORD()
        check("正式版 TSF 啟用", t.call(manager, 14, [c.POINTER(t.DWORD), t.DWORD], c.byref(client), 4))
        document, context, cookie = t.PTR(), t.PTR(), t.DWORD()
        store = t.MemoryTextStore()
        check("正式版文件", t.call(manager, 5, [t.PPTR], c.byref(document)))
        check("正式版文字區", t.call(document, 3, [t.DWORD, t.DWORD, t.PTR, t.PPTR, c.POINTER(t.DWORD)], client.value, 0, store.ptr, c.byref(context), c.byref(cookie)))
        check("正式版推入文字區", t.call(document, 4, [t.PTR], context))
        check("正式版焦點", t.call(manager, 8, [t.PTR], document))
        check("正式版設定檔啟用", t.call(profiles, 3, [t.DWORD, c.c_uint16, c.POINTER(t.Guid), c.POINTER(t.Guid), t.PTR, t.DWORD], 1, 0x0404, c.byref(t.OUR_CLSID), c.byref(t.PROFILE), None, 0x10000004))
        if not system_proxy.processor:
            raise RuntimeError("TSF 未建立已登錄的正式版文字服務")
        hr, sink = t.query(system_proxy.processor, t.IKEYEVENTSINK)
        check("正式版按鍵介面", hr)
        hr, bar = t.query(manager, t.guid("ba468c55-9956-4fb1-a59d-52a7dd7cc6aa"))
        check("正式版語言列介面", hr)
        mode, value = t.PTR(), t.PTR()
        mode_guid = t.guid("2c77a81e-41cc-4178-a3a7-5f8a987568e6")
        check("正式版中英項目", t.call(bar, 4, [c.POINTER(t.Guid), t.PPTR], c.byref(mode_guid), c.byref(mode)))
        check("正式版目前模式", t.call(mode, 11, [t.PPTR], c.byref(value)))
        old_ascii = c.wstring_at(value, oleauto.SysStringLen(value)) == "英"
        oleauto.SysFreeString(value)
        t.release(mode)
        t.release(bar)

        def toggle_system_mode():
            event(4, 0x10)
            if event(5, 0x10) != 1 or event(7, 0x10) != 0:
                raise RuntimeError("正式版無法切換中英模式")

        if old_ascii:
            toggle_system_mode()
        try:
            press(ord("A"))
            press(ord("2"))
            if store.content() != "曰":
                raise RuntimeError("已註冊的輸入法未能在沙盒內查字及上屏")
        finally:
            if old_ascii:
                toggle_system_mode()
        t.release(sink)
        check("正式版清除焦點", t.call(manager, 8, [t.PTR], None))
        check("正式版移除文字區", t.call(document, 5, [t.DWORD], 1))
        t.release(context)
        t.release(document)
        check("正式版 TSF 停用", t.call(manager, 4, []))
        t.release(system_proxy.processor)
        t.release(manager)
        t.release(profiles)
        check("撤銷正式版測試代理", ole.CoRevokeClassObject(registration.value))
        t.release(system_factory)
        print("PASS: 沙盒可透過 Windows COM 登錄載入正式 DLL、使用實際字庫服務輸入；私人字典仍不能直接讀取。", flush=True)
    ole.CoUninitialize()
    print("PASS: AppContainer TSF、私人詞條與多行、檔案隔離、共用狀態、字典重載及斷線恢復。", flush=True)


if __name__ == "__main__":
    main()
