"""在獨立程序中，以 Windows 真正的 TSF 管理員與記憶體文字儲存驗證正式 DLL。"""
import ctypes as c
from pathlib import Path
import re
import shutil
import subprocess
import sys
import tempfile
import uuid
import xml.etree.ElementTree as ET

HR = c.c_long
DWORD = c.c_uint32
LONG = c.c_int32
ULONG = c.c_uint32
PTR = c.c_void_p
PPTR = c.POINTER(PTR)
WIN = c.WINFUNCTYPE
S_OK = 0
E_NOINTERFACE = c.c_long(0x80004002).value
E_NOTIMPL = c.c_long(0x80004001).value
E_FAIL = c.c_long(0x80004005).value


class Guid(c.Structure):
    _fields_ = [("a", DWORD), ("b", c.c_uint16), ("c", c.c_uint16), ("d", c.c_ubyte * 8)]


class Obj(c.Structure):
    _fields_ = [("vtable", c.POINTER(PTR))]


class Selection(c.Structure):
    _fields_ = [("start", LONG), ("end", LONG), ("active", LONG), ("interim", LONG)]


class Status(c.Structure):
    _fields_ = [("dynamic", DWORD), ("static", DWORD)]


class Change(c.Structure):
    _fields_ = [("start", LONG), ("old_end", LONG), ("new_end", LONG)]


class Run(c.Structure):
    _fields_ = [("count", ULONG), ("kind", LONG)]


class VariantData(c.Union):
    _fields_ = [("long", LONG), ("storage", c.c_size_t * 2)]


class Variant(c.Structure):
    _fields_ = [("kind", c.c_uint16), ("reserved", c.c_uint16 * 3), ("data", VariantData)]


def guid(text):
    return Guid.from_buffer_copy(uuid.UUID(text).bytes_le)


def hr_text(hr):
    return f"0x{hr & 0xffffffff:08X}"


def call(obj, index, types, *args):
    raw = c.cast(obj, PTR)
    table = c.cast(raw, c.POINTER(c.POINTER(PTR))).contents
    return WIN(HR, PTR, *types)(table[index])(raw, *args)


def checked(label, hr):
    print(f"{label}: {hr_text(hr)}", flush=True)
    if hr < 0:
        raise RuntimeError(f"{label} failed: {hr_text(hr)}")
    return hr


def query(obj, iid):
    result = PTR()
    hr = call(obj, 0, [c.POINTER(Guid), PPTR], c.byref(iid), c.byref(result))
    if hr < 0:
        return hr, None
    return hr, result


def release(obj):
    if obj:
        return call(obj, 2, [])
    return 0


def verify_mode_menu(button):
    entries = []

    @WIN(HR, PTR, DWORD, DWORD, PTR, PTR, PTR, ULONG, PPTR)
    def add_item(_menu, item_id, flags, _bitmap, _mask, text, length, _submenu):
        entries.append((item_id, flags, c.wstring_at(text, length)))
        return S_OK

    qi = WIN(HR, PTR, c.POINTER(Guid), PPTR)(lambda *_: E_NOINTERFACE)
    ref = WIN(ULONG, PTR)(lambda _: 1)
    table = (PTR * 4)(*(c.cast(fn, PTR) for fn in (qi, ref, ref, add_item)))
    menu = Obj(table)
    checked("TSF mode menu", call(button, 8, [PTR], c.byref(menu)))
    labels = [label for _, _, label in entries]
    expected = [
        "開啟資料目錄", "開啟程式目錄", "", "檢查詞庫更新", "定期更新設定…", "",
        "重新啟動輸入法", "指定程式重啟設定…", "",
        "詳細日誌", "匯出日誌…", "清空 Oxide IME 日誌…", "", "關於 Oxide IME",
    ]
    if "--private-discord" in sys.argv:
        expected.insert(5, "輸入 Discord Webhook 網址…")
    assert labels == expected
    assert [i for i, (_, flags, _) in enumerate(entries) if flags == 4] == [
        i for i, label in enumerate(expected) if not label
    ]
    channel = subprocess.run(["wevtutil.exe", "gl", "OxideIme/Diagnostic", "/f:xml"], capture_output=True)
    enabled = channel.returncode == 0 and ET.fromstring(channel.stdout).get("enabled") == "true"
    assert entries[labels.index("詳細日誌")][1] == int(enabled)
    assert call(button, 9, [DWORD], entries[2][0]) == c.c_long(0x80070057).value
    print("PASS: TSF 右鍵選單的項目、分隔及詳細日誌勾號。", flush=True)


IUNKNOWN = guid("00000000-0000-0000-c000-000000000046")
ICLASSFACTORY = guid("00000001-0000-0000-c000-000000000046")
ITEXTSTOREACP = guid("28888fe3-c2a0-483a-a3ea-8cb1ce51ff3d")
ITEXTSTOREACPSINK = guid("22d44c94-a419-4542-a272-ae26093ececf")
ITEXTINPUTPROCESSOR = guid("aa80e7f7-2021-11d2-93e0-0060b067b86e")
IKEYEVENTSINK = guid("aa80e7f5-2021-11d2-93e0-0060b067b86e")
OUR_CLSID = guid("4bd0700e-5dd1-4310-a9d2-f3d579d54730")
PROFILE = guid("21a75529-20a1-4cf2-b7d8-1bb94267e1c9")
EDIT_SESSION = guid("aa80e803-2021-11d2-93e0-0060b067b86e")
DISPLAY_PROPERTY = guid("34b45670-7526-11d2-a147-00105a2799b5")
INPUT_ATTRIBUTE = guid("240e3ecc-a260-41cf-985a-e17252572803")


class UiElementSink:
    """模擬自行繪製候選的宿主，記錄 TSF 的生命週期通知。"""

    def __init__(self):
        self.refs = 1
        self.opened = []
        self.updated = []
        self.closed = []
        methods = [
            (HR, [c.POINTER(Guid), PPTR], self.query),
            (ULONG, [], self.add_ref),
            (ULONG, [], self.release),
            (HR, [DWORD, c.POINTER(LONG)], self.begin),
            (HR, [DWORD], self.update),
            (HR, [DWORD], self.end),
        ]
        self.callbacks = [WIN(restype, PTR, *args)(fn) for restype, args, fn in methods]
        self.table = (PTR * len(methods))(*[c.cast(fn, PTR).value for fn in self.callbacks])
        self.object = Obj(c.cast(self.table, c.POINTER(PTR)))
        self.ptr = c.cast(c.pointer(self.object), PTR)

    def query(self, this, iid, out):
        if bytes(iid.contents) not in (bytes(IUNKNOWN), bytes(guid("ea1ea136-19df-11d7-a6d2-00065b84435c"))):
            out[0] = None
            return E_NOINTERFACE
        out[0] = this
        self.refs += 1
        return S_OK

    def add_ref(self, this):
        self.refs += 1
        return self.refs

    def release(self, this):
        self.refs -= 1
        return self.refs

    def begin(self, this, element_id, show):
        self.opened.append(element_id)
        show[0] = 0
        return S_OK

    def update(self, this, element_id):
        self.updated.append(element_id)
        return S_OK

    def end(self, this, element_id):
        self.closed.append(element_id)
        return S_OK


class ReadDisplayAttribute:
    """由 TSF 真正的讀取工作階段檢查第一個字的顯示屬性。"""
    def __init__(self, context, offset=0):
        self.context = context
        self.offset = offset
        self.refs = 1
        self.result = None
        self.error = None
        methods = [
            (HR, [c.POINTER(Guid), PPTR], self.query),
            (ULONG, [], self.add_ref),
            (ULONG, [], self.release),
            (HR, [DWORD], self.read),
        ]
        self.callbacks = [WIN(restype, PTR, *args)(fn) for restype, args, fn in methods]
        self.table = (PTR * len(methods))(*[c.cast(fn, PTR).value for fn in self.callbacks])
        self.obj = Obj(c.cast(self.table, c.POINTER(PTR)))
        self.ptr = c.cast(c.pointer(self.obj), PTR)

    def query(self, this, iid, out):
        out[0] = None
        if bytes(iid.contents) not in (bytes(IUNKNOWN), bytes(EDIT_SESSION)):
            return E_NOINTERFACE
        out[0] = this
        self.refs += 1
        return S_OK

    def add_ref(self, this):
        self.refs += 1
        return self.refs

    def release(self, this):
        self.refs -= 1
        return self.refs

    def read(self, this, cookie):
        prop, text_range = PTR(), PTR()
        try:
            checked("Get display property", call(self.context, 12, [c.POINTER(Guid), PPTR], c.byref(DISPLAY_PROPERTY), c.byref(prop)))
            checked("Get start", call(self.context, 7, [DWORD, PPTR], cookie, c.byref(text_range)))
            moved = LONG()
            checked("Expand range", call(text_range, 9, [DWORD, LONG, c.POINTER(LONG), PTR], cookie, self.offset + 1, c.byref(moved), None))
            if moved.value != self.offset + 1:
                raise RuntimeError("無法讀取指定字元的顯示屬性")
            if self.offset:
                checked("Shift range start", call(text_range, 8, [DWORD, LONG, c.POINTER(LONG), PTR], cookie, self.offset, c.byref(moved), None))
                if moved.value != self.offset:
                    raise RuntimeError("無法定位指定字元")
            value = Variant()
            checked("Get display value", call(prop, 5, [DWORD, PTR, c.POINTER(Variant)], cookie, text_range, c.byref(value)))
            self.result = (value.kind, value.data.long)
            return S_OK
        except Exception as error:
            self.error = error
            return E_FAIL
        finally:
            release(text_range)
            release(prop)

    def get(self, client):
        result = HR()
        checked("Request display read", call(self.context, 3, [DWORD, PTR, DWORD, c.POINTER(HR)], client, self.ptr, 3, c.byref(result)))
        if self.error:
            raise self.error
        checked("Display read result", result.value)
        if self.result is None:
            raise RuntimeError("顯示屬性讀取工作階段未執行")
        return self.result


class MemoryTextStore:
    """測試專用 ITextStoreACP；只存放測試字串，不觸及使用者文件。"""

    def __init__(self):
        self.refs = 1
        self.sink = None
        self.locked = 0
        self.text = []
        self.start = 0
        self.end = 0
        self.write_count = 0
        self.lock_id = 0
        self.writes = []
        self.fail_lock_once = False
        self.defer_write_once = False
        self.deferred_lock = 0
        self.fail_write_once = False
        self.fail_nonempty_write_once = False
        self.callbacks = []
        methods = [
            (HR, [c.POINTER(Guid), PPTR], self.query),
            (ULONG, [], self.add_ref),
            (ULONG, [], self.release),
            (HR, [c.POINTER(Guid), PTR, DWORD], self.advise),
            (HR, [PTR], self.unadvise),
            (HR, [DWORD, c.POINTER(HR)], self.request_lock),
            (HR, [c.POINTER(Status)], self.status),
            (HR, [LONG, LONG, ULONG, c.POINTER(LONG), c.POINTER(LONG)], self.query_insert),
            (HR, [ULONG, ULONG, c.POINTER(Selection), c.POINTER(ULONG)], self.get_selection),
            (HR, [ULONG, c.POINTER(Selection)], self.set_selection),
            (HR, [LONG, LONG, PTR, ULONG, c.POINTER(ULONG), c.POINTER(Run), ULONG,
                  c.POINTER(ULONG), c.POINTER(LONG)], self.get_text),
            (HR, [DWORD, LONG, LONG, PTR, ULONG, c.POINTER(Change)], self.set_text),
            (HR, [LONG, LONG, PPTR], self.unimplemented),
            (HR, [LONG, c.POINTER(Guid), c.POINTER(Guid), PPTR], self.unimplemented),
            (HR, [c.POINTER(Guid), PTR, c.POINTER(LONG)], self.unimplemented),
            (HR, [DWORD, LONG, LONG, PTR, c.POINTER(Change)], self.unimplemented),
            (HR, [DWORD, PTR, ULONG, c.POINTER(LONG), c.POINTER(LONG), c.POINTER(Change)], self.insert_text),
            (HR, [DWORD, PTR, c.POINTER(LONG), c.POINTER(LONG), c.POINTER(Change)], self.unimplemented),
            (HR, [DWORD, ULONG, PTR], self.unimplemented),
            (HR, [LONG, ULONG, PTR, DWORD], self.unimplemented),
            (HR, [LONG, ULONG, PTR, DWORD], self.unimplemented),
            (HR, [LONG, LONG, ULONG, PTR, DWORD, c.POINTER(LONG), c.POINTER(LONG), c.POINTER(LONG)], self.unimplemented),
            (HR, [ULONG, PTR, c.POINTER(ULONG)], self.unimplemented),
            (HR, [c.POINTER(LONG)], self.get_end),
            (HR, [c.POINTER(DWORD)], self.get_active_view),
            (HR, [DWORD, PTR, DWORD, c.POINTER(LONG)], self.unimplemented),
            (HR, [DWORD, LONG, LONG, PTR, c.POINTER(LONG)], self.unimplemented),
            (HR, [DWORD, PTR], self.unimplemented),
            (HR, [DWORD, PPTR], self.get_window),
        ]
        for restype, args, function in methods:
            self.callbacks.append(WIN(restype, PTR, *args)(function))
        self.table = (PTR * len(methods))(*[c.cast(fn, PTR).value for fn in self.callbacks])
        self.obj = Obj(c.cast(self.table, c.POINTER(PTR)))
        self.ptr = c.cast(c.pointer(self.obj), PTR)

    def query(self, this, iid, out):
        out[0] = None
        if bytes(iid.contents) not in (bytes(IUNKNOWN), bytes(ITEXTSTOREACP)):
            return E_NOINTERFACE
        out[0] = this
        self.refs += 1
        return S_OK

    def add_ref(self, this):
        self.refs += 1
        return self.refs

    def release(self, this):
        self.refs -= 1
        return self.refs

    def advise(self, this, iid, sink, mask):
        hr, found = query(sink, ITEXTSTOREACPSINK)
        if hr < 0:
            return hr
        if self.sink:
            release(self.sink)
        self.sink = found
        return S_OK

    def unadvise(self, this, sink):
        if self.sink:
            release(self.sink)
            self.sink = None
        return S_OK

    def request_lock(self, this, flags, session):
        if not self.sink or self.locked:
            return E_FAIL
        if self.defer_write_once and flags & 7 == 7:
            self.defer_write_once = False
            self.deferred_lock = flags & ~1  # 宿主稍後再以非同步鎖通知 TSF。
            session[0] = 0x00040300  # TS_S_ASYNC
            return c.c_int32(0x8000ffff).value  # E_UNEXPECTED
        if self.fail_lock_once:
            self.fail_lock_once = False
            session[0] = E_FAIL
            return S_OK
        self.locked = flags
        self.lock_id += 1
        session[0] = call(self.sink, 8, [DWORD], flags)
        self.locked = 0
        if getattr(self, "rewind_to", None) is not None:
            self.start = self.end = self.rewind_to
            self.rewind_to = None
            checked("Host updates caret after lock", call(self.sink, 4, []))
        return S_OK

    def status(self, this, result):
        result[0] = Status(0, 0x8 | 0x80)
        return S_OK

    def query_insert(self, this, start, end, count, out_start, out_end):
        out_start[0], out_end[0] = start, end
        return S_OK

    def get_selection(self, this, index, count, result, fetched):
        if count:
            result[0] = Selection(self.start, self.end, 2, 0)
            fetched[0] = 1
        else:
            fetched[0] = 0
        return S_OK

    def set_selection(self, this, count, selections):
        if count != 1:
            return E_FAIL
        self.start, self.end = selections[0].start, selections[0].end
        return S_OK

    def get_text(self, this, start, end, buffer, capacity, written, runs, run_capacity, run_count, next_pos):
        end = len(self.text) if end < 0 else min(end, len(self.text))
        chunk = self.text[start:end][:capacity] if buffer else []
        if buffer:
            data = c.cast(buffer, c.POINTER(c.c_uint16))
            for index, item in enumerate(chunk):
                data[index] = item
        if written:
            written[0] = len(chunk)
        if run_count:
            run_count[0] = 1 if runs and run_capacity and chunk else 0
        if runs and run_capacity and chunk:
            runs[0] = Run(len(chunk), 0)
        if next_pos:
            next_pos[0] = start + len(chunk)
        return S_OK

    def replace(self, start, end, text, count, change):
        chars = list(c.cast(text, c.POINTER(c.c_uint16))[:count]) if count else []
        self.text[start:end] = chars
        self.start = self.end = start + len(chars)
        self.write_count += 1
        self.writes.append((self.lock_id, len(chars)))
        if getattr(self, "rewind_multiline", False) and 10 in chars:
            self.rewind_multiline = False
            self.rewind_to = start
        if change:
            change[0] = Change(start, end, start + len(chars))

    def reject_write(self, count):
        if self.fail_write_once:
            self.fail_write_once = False
            return True
        if count and self.fail_nonempty_write_once:
            self.fail_nonempty_write_once = False
            return True
        return False

    def wrote_payload_once(self, start, text):
        writes = self.writes[start:]
        units = len(text.encode("utf-16-le")) // 2
        if len(writes) == 1:
            return writes[0][1] == units
        return (len(writes) == 2 and writes[0][1] == 0 and
                writes[1][1] == units and writes[0][0] != writes[1][0])

    def set_text(self, this, flags, start, end, text, count, change):
        if self.reject_write(count):
            return E_FAIL
        self.replace(start, end, text, count, change)
        return S_OK

    def insert_text(self, this, flags, text, count, start_out, end_out, change):
        start, end = self.start, self.end
        if not flags & 0x2 and self.reject_write(count):
            return E_FAIL
        if start_out:
            start_out[0] = start
        if end_out:
            end_out[0] = start + count
        if not flags & 0x2:
            self.replace(start, end, text, count, change)
        return S_OK

    def get_end(self, this, out):
        out[0] = len(self.text)
        return S_OK

    def get_active_view(self, this, out):
        out[0] = 1
        return S_OK

    def get_window(self, this, view, out):
        out[0] = None
        return S_OK

    def unimplemented(self, *args):
        return E_NOTIMPL

    def content(self):
        return bytes((c.c_uint16 * len(self.text))(*self.text)).decode("utf-16-le")


class FactoryProxy:
    """攔截測試程序內的類別工廠，取得 TSF 啟用的正式服務物件。"""

    def __init__(self, real):
        self.real = real
        self.refs = 1
        self.processor = None
        methods = [
            (HR, [c.POINTER(Guid), PPTR], self.query),
            (ULONG, [], self.add_ref),
            (ULONG, [], self.release),
            (HR, [PTR, c.POINTER(Guid), PPTR], self.create),
            (HR, [LONG], self.lock),
        ]
        self.callbacks = [WIN(restype, PTR, *args)(fn) for restype, args, fn in methods]
        self.table = (PTR * len(methods))(*[c.cast(fn, PTR).value for fn in self.callbacks])
        self.obj = Obj(c.cast(self.table, c.POINTER(PTR)))
        self.ptr = c.cast(c.pointer(self.obj), PTR)

    def query(self, this, iid, out):
        out[0] = None
        if bytes(iid.contents) not in (bytes(IUNKNOWN), bytes(ICLASSFACTORY)):
            return E_NOINTERFACE
        out[0] = this
        self.refs += 1
        return S_OK

    def add_ref(self, this):
        self.refs += 1
        return self.refs

    def release(self, this):
        self.refs -= 1
        return self.refs

    def create(self, this, outer, iid, out):
        hr = call(self.real, 3, [PTR, c.POINTER(Guid), PPTR], outer, iid, out)
        if hr >= 0 and out[0]:
            qi_hr, processor = query(out[0], ITEXTINPUTPROCESSOR)
            if qi_hr >= 0:
                if self.processor:
                    release(self.processor)
                self.processor = processor
        return hr

    def lock(self, this, lock):
        return call(self.real, 4, [LONG], lock)


def create_com(ole, class_text, interface_text):
    clsid, iid = guid(class_text), guid(interface_text)
    out = PTR()
    checked("CoCreateInstance", ole.CoCreateInstance(c.byref(clsid), None, 1, c.byref(iid), c.byref(out)))
    return out


def run_with_fixture(dll_path):
    """使用合成詞條測試長文和重載，不複製使用者的自訂字典。"""
    data_dir = dll_path.parent.parent / "data" / "dictionaries"
    workspace = (Path(__file__).resolve().parent.parent / "work").resolve()
    workspace.mkdir(exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="tsf-fixture-", dir=workspace) as directory:
        fixture = Path(directory).resolve()
        if fixture.parent != workspace:
            raise RuntimeError("測試副本不在 work 目錄內")
        (fixture / "data" / "dictionaries").mkdir(parents=True)
        (fixture / "outputs").mkdir()
        shutil.copy2(data_dir / "cangjie3.dict.yaml", fixture / "data" / "dictionaries")
        if re.fullmatch(r"oxide_ime_v\d+\.\d+(?:\.\d+)?(?:_x86)?\.dll", dll_path.name.lower()):
            templates = Path(__file__).resolve().parent.parent / "assets" / "templates"
            dictionaries = fixture / "data" / "dictionaries"
            shutil.copy2(templates / "jyut6ping3.chars.dict.csv.json", dictionaries)
            shutil.copytree(templates / "t2s", dictionaries / "t2s")
        copied_custom = fixture / "data" / "dictionaries" / "cangjie3_custom.dict.yaml"
        long_text = ("測" * 140 + "。\\n") * 6 + "😀" + "試" * 32 + "。"
        code = "z" * 20
        if "--long-text" in sys.argv:
            source = Path(sys.argv[sys.argv.index("--long-text") + 1]).read_text(encoding="utf-8").rstrip("\n")
            long_text = source.replace("\\", "\\\\").replace("\n", "\\n")
            code = "zs"
        markers = ""
        if "--lua-fixture" in sys.argv:
            if b"RustImeSyntheticClipboardFixture" not in dll_path.read_bytes():
                raise RuntimeError("Lua 功能測試必須使用合成剪貼簿測試 DLL")
            markers = ("__rime_clipboard_raw__\tzv\n__rime_clipboard_s2t__\tzv\n"
                       "__rime_time__\tztime\n__rime_date__\tzdate\n")
            conversion = fixture / "data" / "dictionaries" / "s2t"
            conversion.mkdir()
            (conversion / "custom.json").write_text('{"简体":"繁體"}', encoding="utf-8")
            (conversion / "s2t-char.min.json").write_text('{"简":"簡","体":"體"}', encoding="utf-8")
            (conversion / "s2t-phrase.min.json").write_text("{}", encoding="utf-8")
        copied_custom.write_text("---\n...\n" + long_text + "\t" + code + "\n" + markers, encoding="utf-8")
        copied_dll = fixture / "outputs" / dll_path.name
        shutil.copy2(dll_path, copied_dll)
        print("使用 work 內的隔離碼表及合成詞條進行測試。", flush=True)
        subprocess.run([sys.executable, str(Path(__file__).resolve()), str(copied_dll), *sys.argv[2:], "--test-fixture"], check=True)


def release_dll():
    root = Path(__file__).resolve().parent.parent
    version = (root / "release-version.txt").read_text(encoding="utf-8").strip()
    if not re.fullmatch(r"v[1-9][0-9]*\.[0-9]+(?:\.[0-9]+)?", version):
        raise ValueError("release-version.txt 的發行版本無效")
    return root / "outputs" / f"oxide_ime_{version}.dll"


def main():
    dll_path = Path(sys.argv[1] if len(sys.argv) > 1 else release_dll()).resolve()
    version = re.fullmatch(r"(?:rust|oxide)_ime_stage(\d+)(?:_x86)?\.dll", dll_path.name.lower())
    formal = re.fullmatch(r"oxide_ime_v(\d+)\.\d+(?:\.\d+)?(?:_x86)?\.dll", dll_path.name.lower())
    ranked = formal and int(formal.group(1)) >= 2
    completion = "時" if ranked else "昌"
    moon_completion = "同" if ranked else "㸓"
    # 正式版涵蓋各開發階段已交付的功能。保留舊版作回歸對照。
    stage = 54 if formal else int(version.group(1)) if version else 0
    stage5 = "--stage5" in sys.argv or stage >= 5
    stage3 = stage5 or "--stage3" in sys.argv or stage >= 3
    if "--test-fixture" not in sys.argv:
        run_with_fixture(dll_path)
        return
    ole = c.WinDLL("ole32")
    ole.CoInitializeEx.argtypes = [PTR, DWORD]
    ole.CoInitializeEx.restype = HR
    ole.CoCreateInstance.argtypes = [c.POINTER(Guid), PTR, DWORD, c.POINTER(Guid), PPTR]
    ole.CoCreateInstance.restype = HR
    ole.CoRegisterClassObject.argtypes = [c.POINTER(Guid), PTR, DWORD, DWORD, c.POINTER(DWORD)]
    ole.CoRegisterClassObject.restype = HR
    checked("CoInitializeEx", ole.CoInitializeEx(None, 2))
    dll = c.WinDLL(str(dll_path))
    dll.DllGetClassObject.argtypes = [c.POINTER(Guid), c.POINTER(Guid), PPTR]
    dll.DllGetClassObject.restype = HR
    factory = PTR()
    checked("Release DllGetClassObject", dll.DllGetClassObject(c.byref(OUR_CLSID), c.byref(ICLASSFACTORY), c.byref(factory)))
    proxy = FactoryProxy(factory)
    registration = DWORD()
    checked("CoRegisterClassObject", ole.CoRegisterClassObject(c.byref(OUR_CLSID), proxy.ptr, 1, 1, c.byref(registration)))

    manager = create_com(ole, "529a9e6b-6587-4f23-ab9e-9c7d683e3c50", "aa80e801-2021-11d2-93e0-0060b067b86e")
    profiles = create_com(ole, "33c53a50-f456-4884-b049-85fd643ecfed", "71c6e74c-0f28-11d8-a82a-00065b84435c")
    client = DWORD()
    checked("ThreadMgr.Activate", call(manager, 3, [c.POINTER(DWORD)], c.byref(client)))
    document = PTR()
    checked("CreateDocumentMgr", call(manager, 5, [PPTR], c.byref(document)))
    store = MemoryTextStore()
    context = PTR()
    cookie = DWORD()
    checked("CreateContext", call(document, 3, [DWORD, DWORD, PTR, PPTR, c.POINTER(DWORD)], client.value, 0, store.ptr, c.byref(context), c.byref(cookie)))
    checked("Push", call(document, 4, [PTR], context))
    checked("SetFocus", call(manager, 8, [PTR], document))
    checked("ActivateProfile(for process)", call(profiles, 3, [DWORD, c.c_uint16, c.POINTER(Guid), c.POINTER(Guid), PTR, DWORD], 1, 0x0404, c.byref(OUR_CLSID), c.byref(PROFILE), None, 0x10000004))
    if not proxy.processor:
        raise RuntimeError("TSF 未建立正式版文字服務物件")
    hr, sink = query(proxy.processor, IKEYEVENTSINK)
    checked("Query key sink", hr)
    mode_button = None
    if stage3:
        hr, bar = query(manager, guid("ba468c55-9956-4fb1-a59d-52a7dd7cc6aa"))
        checked("Query language bar manager", hr)
        item = PTR()
        mode_guid = guid("2c77a81e-41cc-4178-a3a7-5f8a987568e6")
        checked("Find input-mode item", call(bar, 4, [c.POINTER(Guid), PPTR], c.byref(mode_guid), c.byref(item)))
        hr, mode_button = query(item, guid("28c7f1d0-de25-11d2-afdd-00105a2799b5"))
        checked("Query input-mode button", hr)
        release(item)
        release(bar)
        oleauto = c.WinDLL("oleaut32")
        oleauto.SysStringLen.argtypes = [PTR]
        oleauto.SysStringLen.restype = ULONG
        oleauto.SysFreeString.argtypes = [PTR]
        user32 = c.WinDLL("user32")
        user32.DestroyIcon.argtypes = [PTR]

        def mode_text():
            value = PTR()
            checked("Get mode text", call(mode_button, 11, [PPTR], c.byref(value)))
            text = c.wstring_at(value, oleauto.SysStringLen(value))
            oleauto.SysFreeString(value)
            icon = PTR()
            checked("Get mode icon", call(mode_button, 10, [PPTR], c.byref(icon)))
            if not icon:
                raise RuntimeError("輸入模式圖示為空")
            user32.DestroyIcon(icon)
            return text
        if mode_text() != "英":
            raise RuntimeError("輸入模式初始圖示不正確")
        if ranked:
            verify_mode_menu(mode_button)
    def key_event(slot, key):
        eaten = LONG()
        hr = call(sink, slot, [PTR, c.c_size_t, c.c_ssize_t, c.POINTER(LONG)], context, key, 0, c.byref(eaten))
        checked(f"key event {slot}", hr)
        return eaten.value

    def press(key):
        wants = key_event(4, key)
        if wants != 1:
            raise RuntimeError(f"按鍵 {key:#x} 未被測試為可處理")
        handled = key_event(6, key)
        if handled != 1:
            raise RuntimeError(f"按鍵 {key:#x} 實際未被處理")
        if key_event(5, key):
            key_event(7, key)

    def toggle():
        if key_event(4, 0x10) != 0 or key_event(5, 0x10) != 1 or key_event(7, 0x10) != 0:
            raise RuntimeError("Shift 切換或按鍵釋放不符合預期")

    toggle()
    if mode_button and mode_text() != "中":
        raise RuntimeError("Shift 後輸入模式未同步更新")
    # 拿不到編輯鎖時不得吞掉按鍵，也不能留下半成品組字。
    store.fail_lock_once = True
    if key_event(4, 0x41) != 1 or key_event(6, 0x41) != 0 or store.content():
        raise RuntimeError("編輯鎖失敗後的狀態或按鍵處理不正確")
    if store.fail_lock_once:
        raise RuntimeError("未觸發預期的編輯鎖失敗")
    # 第一筆寫入失敗亦須維持原文，並容許下一次組字。
    store.fail_write_once = True
    if key_event(4, 0x41) != 1 or key_event(6, 0x41) != 0 or store.content():
        raise RuntimeError("首次文字寫入失敗後的狀態不正確")
    if store.fail_write_once:
        raise RuntimeError("未觸發預期的文字寫入失敗")
    press(0x41)
    if store.content() != "日":
        raise RuntimeError("組字預編輯文字不正確")
    if stage >= 18:
        kind, atom = ReadDisplayAttribute(context).get(client.value)
        if kind != 3 or not atom:
            raise RuntimeError("組字文字未套用 TSF 顯示屬性")
        category = create_com(ole, "a4b544a1-438d-4b41-9325-869523e2d6c7", "c3acefb5-f69d-4905-938f-fcadcf4be830")
        resolved = Guid()
        checked("Resolve display atom", call(category, 15, [DWORD, c.POINTER(Guid)], atom, c.byref(resolved)))
        release(category)
        if bytes(resolved) != bytes(INPUT_ATTRIBUTE):
            raise RuntimeError("組字顯示屬性並非輸入法的底線屬性")
    # 既有組字的候選上屏失敗後，可再次選字；原組字不應被清除。
    store.fail_write_once = True
    if key_event(4, 0x32) != 1 or key_event(6, 0x32) != 0 or store.content() != "日":
        raise RuntimeError("候選寫入失敗時未保留原組字")
    if store.fail_write_once:
        raise RuntimeError("未觸發預期的候選寫入失敗")
    if stage >= 18 and ReadDisplayAttribute(context).get(client.value) != (3, atom):
        raise RuntimeError("候選寫入失敗後，原本的組字底線不應消失")
    press(0x32)
    if store.content() != "曰":
        raise RuntimeError("第二候選提交不正確")
    if stage >= 18:
        kind, _ = ReadDisplayAttribute(context).get(client.value)
        if kind != 0:
            raise RuntimeError("候選提交後仍保留組字底線屬性")
        print("PASS: TSF 組字底線屬性已套用，提交後已清除。", flush=True)
    print("編輯鎖及寫入失敗：原文字與組字狀態均正確保留。", flush=True)
    store.start, store.end = 0, 1
    checked("OnSelectionChange", call(store.sink, 4, []))
    store.fail_write_once = True
    if key_event(4, 0x41) != 1 or key_event(6, 0x41) != 0 or store.content() != "曰":
        raise RuntimeError("取代選取文字失敗時，原文字未保留")
    if store.fail_write_once:
        raise RuntimeError("未觸發預期的選取文字寫入失敗")
    store.start = store.end = len(store.text)
    checked("OnSelectionChange", call(store.sink, 4, []))
    press(0x41)
    press(0x42)
    if store.content() != "曰日月":
        raise RuntimeError("多碼預編輯不正確")
    if stage >= 18 and any(ReadDisplayAttribute(context, offset).get(client.value) != (3, atom) for offset in (1, 2)):
        raise RuntimeError("多碼組字的每個字根均應保留底線")
    press(0x1b)
    if store.content() != "曰":
        raise RuntimeError("Escape 未取消組字")

    press(0x41)
    if stage5:
        before_blocked = store.content()
        writes = store.write_count
        for key in [0x09, 0x21, 0x22, 0x30, 0x33, *range(0x60, 0x70), 0x73, 0xbe, 0xbf]:
            press(key)
            if store.content() != before_blocked or store.write_count != writes:
                raise RuntimeError(f"組字期間被禁止的按鍵 {key:#x} 改動了文字")
        if key_event(4, 0x14) != 0:
            raise RuntimeError("組字期間 Caps Lock 未交由系統處理")
        press(0x27)
        press(0x27)  # 方向鍵選到下一頁
    else:
        press(0x22)  # PageDown
    press(0x31)
    if store.content() != "曰" + completion:
        raise RuntimeError("翻頁後的候選提交不正確")

    press(0x41)
    press(0x42)
    press(0x08)
    if store.content() != "曰" + completion + "日":
        raise RuntimeError("退格未正確更新組字")
    press(0x08)
    if store.content() != "曰" + completion:
        raise RuntimeError("刪除最後一碼未取消組字")

    store.start, store.end = 0, 1
    checked("OnSelectionChange", call(store.sink, 4, []))
    press(0x41)
    if store.content() != "日" + completion:
        raise RuntimeError("覆蓋選取文字的組字不正確")
    press(0x1b)
    if store.content() != "曰" + completion:
        raise RuntimeError("取消組字未還原選取文字")
    if stage >= 18 and ReadDisplayAttribute(context).get(client.value)[0] != 0:
        raise RuntimeError("取消組字後，還原文字不應保留組字底線")
    store.start = store.end = len(store.text)
    checked("OnSelectionChange", call(store.sink, 4, []))

    press(0x41)
    toggle()
    if store.content() != "曰" + completion + "a" or key_event(4, 0x41) != 0:
        raise RuntimeError("切換英文時未提交原碼或未放行英文字母")
    toggle()

    def dictionary_rows(path):
        source = path.read_text(encoding="utf-8-sig").split("\n...\n", 1)[1]
        return [tuple(line.split("\t")[:2]) for line in source.splitlines()
                if line and not line.startswith("#") and "\t" in line]

    def decode(value):
        if "\\n" not in value:
            return value
        result, i = [], 0
        while i < len(value):
            if value[i] == "\\" and i + 1 < len(value) and value[i + 1] in "n\\":
                result.append("\n" if value[i + 1] == "n" else "\\")
                i += 2
            else:
                result.append(value[i])
                i += 1
        return "".join(result)

    data_dir = dll_path.parent.parent / "data" / "dictionaries"
    base = dictionary_rows(data_dir / "cangjie3.dict.yaml")
    custom = dictionary_rows(data_dir / "cangjie3_custom.dict.yaml")
    long_text, long_code = next((text, code) for text, code in custom if "\\n" in text and len(text) > 20)
    rows = [(text, code, position) for position, (text, code) in enumerate(base + custom)
            if code.startswith(long_code) and all(ord(ch) <= 0xFFFF or 0x1F000 <= ord(ch) <= 0x1FFFF for ch in text)]
    rows.sort(key=lambda row: (len(row[1]), row[1], row[2]))
    candidates, seen = [], set()
    for text, code, _ in rows:
        if text not in seen:
            candidates.append(text)
            seen.add(text)
    index = candidates.index(long_text)
    before = store.content()
    for letter in long_code:
        press(ord(letter.upper()))
    for _ in range(index // 2):
        if stage5:
            press(0x27)
            press(0x27)
        else:
            press(0x22)
    before_write = store.write_count
    press(0x31 + index % 2)
    expected = decode(long_text)
    if store.content() != before + expected or "\n" not in expected:
        raise RuntimeError("自訂長文或換行的完整提交不正確")
    if not store.wrote_payload_once(before_write, expected):
        raise RuntimeError("提交長文未使用單次文字寫入")
    print(f"長文檢查：{len(expected)} 字元、{expected.count(chr(10))} 個換行，已完整以一次寫入提交。")
    if stage >= 42:
        original = store.text[:]
        original_text = store.content()
        store.start = store.end = 1
        checked("Select insertion point with following text", call(store.sink, 4, []))
        for letter in long_code:
            press(ord(letter.upper()))
        for _ in range(index // 2):
            press(0x27)
            press(0x27)
        store.rewind_multiline = True
        before_write = store.write_count
        press(0x31 + index % 2)
        end = 1 + len(expected.encode("utf-16-le")) // 2
        if store.content() != original_text[:1] + expected + original_text[1:]:
            raise RuntimeError("多行插入後，原有後文或候選文字有改動")
        if store.rewind_multiline or (store.start, store.end) != (end, end):
            raise RuntimeError(f"插入點未恢復到多行候選末端，實際 {(store.start, store.end)}，預期 {end}")
        if not store.wrote_payload_once(before_write, expected):
            raise RuntimeError("游標修正不應再次寫入候選文字")
        store.text = original
        store.start = store.end = len(original)
        checked("Restore isolated test document", call(store.sink, 4, []))
        print("PASS: 宿主釋放編輯鎖後重設游標，輸入法將游標放回候選末端並保留原文。")
    surrogate = next((i for i, unit in enumerate(store.text) if 0xD800 <= unit <= 0xDBFF), None)
    if surrogate is not None:
        full_text = store.content()
        if stage >= 36:
            store.start, store.end = surrogate, surrogate + 2
            checked("OnSelectionChange", call(store.sink, 4, []))
            for letter in long_code:
                press(ord(letter.upper()))
            for _ in range(index // 2):
                press(0x27)
                press(0x27)
            store.fail_nonempty_write_once = True
            press(0x31 + index % 2)
            if store.content() != full_text or store.fail_nonempty_write_once:
                raise RuntimeError("多行提交失敗後未完整還原原先選取的非 BMP 字元")
            print("PASS: 多行提交被拒絕時，還原原有 UTF-16 並吸收選字鍵。", flush=True)
        store.start, store.end = surrogate, surrogate + 2
        checked("OnSelectionChange", call(store.sink, 4, []))
        press(0x41)
        press(0x1b)
        if store.content() != full_text:
            raise RuntimeError("取消組字時未完整還原非 BMP 字元")
        store.start = store.end = len(store.text)
        checked("OnSelectionChange", call(store.sink, 4, []))
        print("非 BMP 字元：取消組字後完整還原。", flush=True)

    # 應用程式可終止目前 composition；之後的輸入必須重新開始。
    prefix = store.content()
    press(0x41)
    hr, owner = query(context, guid("86462810-593b-4916-9764-19c08e9ce110"))
    checked("Query owner composition services", hr)
    checked("Host terminates composition", call(owner, 7, [PTR], None))
    release(owner)
    if store.content() != (prefix if stage5 else prefix + "日"):
        raise RuntimeError("宿主終止組字後的文字不正確")
    press(0x42)
    press(0x32)
    if store.content() != prefix + ("" if stage5 else "日") + moon_completion:
        raise RuntimeError("宿主終止組字後未能重新組字")
    print("宿主終止組字：狀態已清理，之後可重新輸入。", flush=True)

    if stage5:
        class Point(c.Structure):
            _fields_ = [("x", LONG), ("y", LONG)]

        class Message(c.Structure):
            _fields_ = [("window", PTR), ("message", DWORD), ("wparam", c.c_size_t),
                        ("lparam", c.c_ssize_t), ("time", DWORD), ("point", Point), ("private", DWORD)]

        user32 = c.WinDLL("user32", use_last_error=True)
        user32.CreateWindowExW.argtypes = [DWORD, c.c_wchar_p, c.c_wchar_p, DWORD, LONG, LONG,
                                           LONG, LONG, PTR, PTR, PTR, PTR]
        user32.CreateWindowExW.restype = PTR
        user32.PostMessageW.argtypes = [PTR, DWORD, c.c_size_t, c.c_ssize_t]
        user32.PeekMessageW.argtypes = [c.POINTER(Message), PTR, DWORD, DWORD, DWORD]
        user32.DispatchMessageW.argtypes = [c.POINTER(Message)]
        user32.DestroyWindow.argtypes = [PTR]
        test_window = user32.CreateWindowExW(0, "STATIC", "", 0x80000000,
                                             0, 0, 20, 20, None, None, None, None)
        if not test_window:
            raise OSError(c.get_last_error(), "無法建立不可見的測試視窗")
        prefix = store.content()
        press(0x41)
        if not user32.PostMessageW(test_window, 0x0201, 0, 0):  # WM_LBUTTONDOWN
            raise OSError(c.get_last_error(), "無法傳送測試滑鼠訊息")
        message = Message()
        if not user32.PeekMessageW(c.byref(message), test_window, 0, 0, 1):
            raise RuntimeError("無法讀取測試滑鼠訊息")
        user32.DispatchMessageW(c.byref(message))
        store.start = store.end = 0
        checked("Host moves selection", call(store.sink, 4, []))
        if store.content() != prefix:
            raise RuntimeError("滑鼠移動文字插入點後未取消未完成的字根")
        if store.start != 0 or store.end != 0:
            raise RuntimeError("取消組字時改變了使用者選擇的插入點")
        store.start = store.end = len(store.text)
        checked("OnSelectionChange", call(store.sink, 4, []))
        user32.DestroyWindow(test_window)

    press(0x41)
    checked("key sink loses focus", call(sink, 3, [LONG], 0))
    if not store.content().endswith(expected + (moon_completion if stage5 else "日" + moon_completion + "a")):
        raise RuntimeError("失去輸入焦點時的未完成組字處理不正確")

    if mode_button:
        prefix = store.content()
        press(0x73)  # F4
        press(0x35 if stage >= 28 else 0x36)  # 中式標點
        press(0x73)
        press(0xbe)  # .
        if store.content() != prefix + "。":
            raise RuntimeError("F4 切換中式標點後未正確寫入")
        press(0xdb)  # [：標點候選
        if not store.content().endswith("。「"):
            raise RuntimeError("中式標點候選未正確組字")
        press(0x32)
        if store.content() != prefix + "。【":
            raise RuntimeError("中式標點選字不正確")
        press(0x73)
        press(0x34 if stage >= 28 else 0x35)  # 全角
        press(0x73)
        press(0x31)
        if not store.content().endswith("。【１"):
            raise RuntimeError("全角數字未正確寫入")
        toggle()
        if mode_text() != "英":
            raise RuntimeError("英文模式圖示未同步更新")
        press(0x41)
        if not store.content().endswith("。【１ａ"):
            raise RuntimeError("英文模式全角字母未正確寫入")
        toggle()
        if mode_text() != "中":
            raise RuntimeError("切回中文時模式圖示未更新")
        press(0x73)
        press(0x34 if stage >= 28 else 0x35)  # 半角
        press(0x31 if stage >= 28 else 0x32)  # 全字
        press(0x32 if stage >= 28 else 0x33)  # 簡體輸出
        if stage < 28:
            press(0x31)  # 舊版的同音選項
        press(0x73)
        for letter in "mls":
            press(ord(letter.upper()))
        press(0x20)
        if not store.content().endswith("。【１ａ𠀀"):
            raise RuntimeError("切換全字後，補充平面漢字未完整上屏")
        for letter in "shike":
            press(ord(letter.upper()))
        press(0x20)
        if not store.content().endswith("𠀀发"):
            raise RuntimeError("簡體模式未將繁體候選轉為簡體")
        press(0x73)
        press(0x32 if stage >= 28 else 0x33)  # 繁體：原文直出
        press(0x73)
        for letter in "shike":
            press(ord(letter.upper()))
        press(0x20)
        if not store.content().endswith("𠀀发髮"):
            raise RuntimeError("繁體模式未保留原文")
        press(0x73)
        press(0x33 if stage >= 28 else 0x34)
        if mode_text() != ("中" if stage >= 35 else "英"):
            raise RuntimeError("F4 設定預設輸入模式後，現行模式不符合版本行為")
        press(0x33 if stage >= 28 else 0x34)
        if mode_text() != "中":
            raise RuntimeError("F4 選擇中文時，輸入模式圖示未同步")
        press(0x73)
        press(0x41)
        checked("Click input-mode icon", call(mode_button, 7, [LONG, c.c_int64, PTR], 2, 0, None))
        if mode_text() != "英" or not store.content().endswith("𠀀发髮a"):
            raise RuntimeError("點擊模式圖示未提交原碼並切換英文")
        print("PASS: F4、中英文狀態及全半角／標點切換。", flush=True)

    if stage5:
        workspace = (Path(__file__).resolve().parent.parent / "work").resolve()
        if "--test-fixture" not in sys.argv or not data_dir.resolve().is_relative_to(workspace):
            raise RuntimeError("字庫重載測試只允許修改 work 內的隔離副本")
        toggle()
        custom_path = data_dir / "cangjie3_custom.dict.yaml"
        original = custom_path.read_bytes()
        separator = b"" if original.endswith(b"\n") else b"\n"
        code = "z" * 24

        def save_entry(value):
            custom_path.write_bytes(original + separator + (value + "\t" + code + "\n").encode("utf-8"))

        def enter_code():
            for letter in code:
                press(ord(letter.upper()))

        save_entry("即時更新甲")
        enter_code()
        save_entry("即時更新的第二版")
        press(0x31)
        if not store.content().endswith("即時更新甲"):
            raise RuntimeError("字庫更新時未保留已顯示的候選")
        enter_code()
        press(0x31)
        if not store.content().endswith("即時更新的第二版"):
            raise RuntimeError("既有程序未重新載入已儲存的字庫")
        custom_path.write_bytes(original + separator + "格式無效\n".encode("utf-8"))
        enter_code()
        press(0x31)
        if not store.content().endswith("即時更新的第二版"):
            raise RuntimeError("字庫格式錯誤時未保留上一份有效字庫")
        save_entry("修復之後的新詞條")
        enter_code()
        press(0x31)
        if not store.content().endswith("修復之後的新詞條"):
            raise RuntimeError("字庫修復後未重新載入")
        print("PASS: 既有 TSF 程序重新載入、保留已顯示候選及處理無效字庫。", flush=True)

    if stage >= 28:
        # SetKeyboardState 只改變本測試執行緒的鍵盤狀態，結束後立即還原。
        user32 = c.WinDLL("user32", use_last_error=True)
        keyboard_type = c.c_ubyte * 256
        user32.GetKeyboardState.argtypes = [c.POINTER(c.c_ubyte)]
        user32.SetKeyboardState.argtypes = [c.POINTER(c.c_ubyte)]

        def ctrl_key(key):
            saved = keyboard_type()
            if not user32.GetKeyboardState(saved):
                raise c.WinError(c.get_last_error())
            modified = keyboard_type(*saved)
            modified[0x11] |= 0x80
            if not user32.SetKeyboardState(modified):
                raise c.WinError(c.get_last_error())
            try:
                press(key)
            finally:
                if not user32.SetKeyboardState(saved):
                    raise c.WinError(c.get_last_error())

        before = store.content()
        for letter in "shike":
            press(ord(letter.upper()))
        preedit = store.content()
        ctrl_key(0xBF)  # 組字中繁 → 簡，不應提交或取消字碼。
        if store.content() != preedit:
            raise RuntimeError("組字中 Ctrl+/ 不應提交或取消字碼")
        press(0x20)
        if store.content() != before + "发":
            raise RuntimeError("組字中 Ctrl+/ 沒有切換至簡體")
        ctrl_key(0xBF)  # 無組字時簡 → 繁。
        for letter in "shike":
            press(ord(letter.upper()))
        press(0x20)
        if store.content() != before + "发髮":
            raise RuntimeError("無組字時 Ctrl+/ 沒有切換至原文直出")
        print("PASS: 真正 TSF 的 Ctrl+/ 可在組字中及無組字時切換繁簡。", flush=True)

    if stage >= 30:
        fuzzy_path = dll_path.parent.parent / "data" / "dictionaries" / "homophone.fuzzy.yaml"

        def reverse_code(code):
            press(0xC0)
            for letter in code:
                press(ord(letter.upper()))

        before = store.content()
        reverse_code("ngoxx")
        if store.content() != before + "ngoxx":
            raise RuntimeError("粵拼組字應顯示英文字母")
        press(0x31)
        first = "㦱" if formal else "我"
        if store.content() != before + first:
            raise RuntimeError("第五聲反查次序與字庫不符")
        press(ord("A"))
        if store.content() != before + first + "日":
            raise RuntimeError("反查選字後未返回倉頡")
        press(0x1B)

        fuzzy_path.write_text("enabled: true\nrules: ['derive/^ngo/xyz/']\n", encoding="utf-8")
        before = store.content()
        reverse_code("xyzxx")
        ctrl_key(0xC0)
        press(0x31)
        if store.content() != before + "xyzxx":
            raise RuntimeError("精確反查不應選取模糊拼法")
        ctrl_key(0xC0)
        press(0x31)
        if store.content() != before + first:
            raise RuntimeError("反查沒有採用已儲存的自訂模糊音")
        print("PASS: 真正 TSF 的粵拼聲調、精確切換、規則重載及返回倉頡。", flush=True)

    if "--lua-fixture" in sys.argv:
        if stage < 28:
            # 舊版前面的 F4 測試關閉過同音，需重新啟用。
            press(0x73)
            press(0x31)
            press(0x73)
        before = store.content()
        for letter in "azx":
            press(ord(letter.upper()))
        press(0x32)
        if store.content() != before + ("㳑" if formal else "逸"):
            raise RuntimeError("TSF 同音完整音的第二候選未正確上屏")
        before = store.content()
        for letter in "zdate":
            press(ord(letter.upper()))
        press(0x31)
        if not re.fullmatch(re.escape(before) + r"\d{2}/\d{2}/\d{4}", store.content()):
            raise RuntimeError("TSF 日期命令格式或上屏不正確")
        before = store.content()
        for letter in "ztime":
            press(ord(letter.upper()))
        press(0x31)
        if not re.fullmatch(re.escape(before) + r"\d{2}:\d{2}:\d{2}", store.content()):
            raise RuntimeError("TSF 時間命令格式或上屏不正確")
        before = store.content()
        for letter in "zv":
            press(ord(letter.upper()))
        writes = store.write_count
        press(0x32)
        if (store.content() != before + "合成繁體\n第二行🙂" or
                not store.wrote_payload_once(writes, "合成繁體\n第二行🙂")):
            raise RuntimeError("TSF 合成剪貼簿候選未以一次提交完整上屏")
        print("PASS: 真正 TSF 同音、日期、時間及合成剪貼簿候選。", flush=True)

    release(sink)
    release(mode_button)
    checked("SetFocus(NULL)", call(manager, 8, [PTR], None))
    checked("Pop", call(document, 5, [DWORD], 1))
    release(context)
    release(document)
    checked("ThreadMgr.Deactivate", call(manager, 4, []))
    release(proxy.processor)
    release(manager)
    release(profiles)
    checked("CoRevokeClassObject", ole.CoRevokeClassObject(registration.value))
    release(factory)
    ole.CoUninitialize()
    print("PASS: 真正的 Windows TSF 已驗證查字、翻頁、選字、退格、取消、中英切換、長文換行及焦點清理。", flush=True)


if __name__ == "__main__":
    main()
