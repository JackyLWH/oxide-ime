"""以真正的 TSF UIElement 專用執行緒測試中文組字。"""
import ctypes as c
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile

sys.dont_write_bytecode = True
import tsf_real_store_test as t


def run_isolated(source, project):
    work = project / "work"
    work.mkdir(exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="tsf-uiless-", dir=work) as directory:
        fixture = Path(directory)
        data = fixture / "data" / "dictionaries"
        outputs = fixture / "outputs"
        data.mkdir(parents=True)
        outputs.mkdir()
        (data / "cangjie3.dict.yaml").write_text("---\n...\n日\ta\n", encoding="utf-8")
        (data / "cangjie3_custom.dict.yaml").write_text("---\n...\n", encoding="utf-8")
        dll = outputs / source.name
        shutil.copy2(source, dll)
        subprocess.run([
            sys.executable, str(Path(__file__).resolve()), str(dll), "--test-fixture",
        ], check=True)


def verify(dll_path, shared=False):
    ole = c.WinDLL("ole32")
    ole.CoInitializeEx.argtypes = [t.PTR, t.DWORD]
    ole.CoInitializeEx.restype = t.HR
    ole.CoCreateInstance.argtypes = [c.POINTER(t.Guid), t.PTR, t.DWORD, c.POINTER(t.Guid), t.PPTR]
    ole.CoCreateInstance.restype = t.HR
    ole.CoRegisterClassObject.argtypes = [c.POINTER(t.Guid), t.PTR, t.DWORD, t.DWORD, c.POINTER(t.DWORD)]
    ole.CoRegisterClassObject.restype = t.HR
    t.checked("CoInitializeEx", ole.CoInitializeEx(None, 2))

    dll = c.WinDLL(str(dll_path))
    dll.DllGetClassObject.argtypes = [c.POINTER(t.Guid), c.POINTER(t.Guid), t.PPTR]
    dll.DllGetClassObject.restype = t.HR
    factory = t.PTR()
    t.checked("Factory", dll.DllGetClassObject(c.byref(t.OUR_CLSID), c.byref(t.ICLASSFACTORY), c.byref(factory)))
    proxy = t.FactoryProxy(factory)
    registration = t.DWORD()
    t.checked("Register class", ole.CoRegisterClassObject(c.byref(t.OUR_CLSID), proxy.ptr, 1, 1, c.byref(registration)))

    manager = t.create_com(ole, "529a9e6b-6587-4f23-ab9e-9c7d683e3c50", "3e90ade3-7594-4cb0-bb58-69628f5f458c")
    client = t.DWORD()
    t.checked("ActivateEx UIElement only", t.call(manager, 14, [c.POINTER(t.DWORD), t.DWORD], c.byref(client), 4))
    flags = t.DWORD()
    t.checked("GetActiveFlags", t.call(manager, 15, [c.POINTER(t.DWORD)], c.byref(flags)))
    if not flags.value & 4:
        raise RuntimeError("TSF 執行緒沒有啟用 UIElement 專用模式。")

    document = t.PTR()
    t.checked("Document", t.call(manager, 5, [t.PPTR], c.byref(document)))
    store = t.MemoryTextStore()
    context, cookie = t.PTR(), t.DWORD()
    t.checked("Context", t.call(document, 3, [t.DWORD, t.DWORD, t.PTR, t.PPTR, c.POINTER(t.DWORD)],
                               client.value, 0, store.ptr, c.byref(context), c.byref(cookie)))
    t.checked("Push", t.call(document, 4, [t.PTR], context))
    t.checked("Focus", t.call(manager, 8, [t.PTR], document))
    profiles = t.create_com(ole, "33c53a50-f456-4884-b049-85fd643ecfed", "71c6e74c-0f28-11d8-a82a-00065b84435c")
    t.checked("ActivateProfile", t.call(profiles, 3, [
        t.DWORD, c.c_uint16, c.POINTER(t.Guid), c.POINTER(t.Guid), t.PTR, t.DWORD,
    ], 1, 0x0404, c.byref(t.OUR_CLSID), c.byref(t.PROFILE), None, 0x10000004))
    if not proxy.processor:
        raise RuntimeError("Windows 未在 UIElement 專用執行緒啟用此文字服務。")
    status, sink = t.query(proxy.processor, t.IKEYEVENTSINK)
    t.checked("Query key sink", status)

    def key(slot, value):
        eaten = t.LONG()
        status = t.call(sink, slot, [t.PTR, c.c_size_t, c.c_ssize_t, c.POINTER(t.LONG)],
                        context, value, 0, c.byref(eaten))
        t.checked(f"Key event {slot}", status)
        return eaten.value

    def shift():
        key(4, 0x10)
        key(5, 0x10)
        key(7, 0x10)

    if shared:
        for value in (*map(ord, "NGO"), 0x20):
            if key(4, value) != 1 or key(6, value) != 1:
                raise RuntimeError("桌面宿主未繼承由沙盒切換的粵拼模式。")
        if store.content() != "我":
            raise RuntimeError("桌面宿主未用繼承的粵拼狀態完成選字。")
        for value in (ord("S"), 0x20):
            if key(4, value) != 1 or key(6, value) != 1:
                raise RuntimeError("桌面宿主未繼承由沙盒切換的中文模式。")
        if store.content() != "我发":
            raise RuntimeError("桌面宿主未繼承由沙盒切換的簡體模式。")
        shift()
        if key(4, ord("A")) != 0:
            raise RuntimeError("桌面宿主無法切回英文模式。")
    else:
        shift()
        if key(4, 0x41) != 1 or key(6, 0x41) != 1 or store.content() != "日":
            raise RuntimeError("UIElement 專用執行緒未能處理中文組字。")
    t.release(sink)
    t.checked("Clear focus", t.call(manager, 8, [t.PTR], None))
    t.checked("Pop", t.call(document, 5, [t.DWORD], 1))
    t.release(context)
    t.release(document)
    t.checked("Deactivate", t.call(manager, 4, []))
    t.release(proxy.processor)
    t.release(manager)
    t.release(profiles)
    ole.CoRevokeClassObject(registration.value)
    t.release(factory)
    ole.CoUninitialize()
    print("PASS: 共用粵拼、倉頡、中英及繁簡模式。" if shared else "PASS: 真正 TSF 的 UIElement 專用執行緒可以啟用中文組字。")


def main():
    dll = Path(sys.argv[1] if len(sys.argv) > 1 else t.release_dll()).resolve()
    if "--test-fixture" in sys.argv:
        verify(dll, "--test-shared" in sys.argv)
    else:
        run_isolated(dll, Path(__file__).resolve().parent.parent)


if __name__ == "__main__":
    main()
