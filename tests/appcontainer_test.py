"""建立臨時 AppContainer，測試隔離資料上的 DLL、查字程序及真正的 TSF。"""
import ctypes as c
import argparse
from ctypes import wintypes as w
from pathlib import Path
import csv
import os
import re
import shutil
import struct
import subprocess
import sys
import tempfile
import time
import uuid
import winreg

ROOT = Path(__file__).resolve().parent.parent
PTR = c.c_void_p
COMMAND_MARKERS = (
    "__rime_clipboard_raw__\tzv\n__rime_clipboard_s2t__\tzv\n"
    "__rime_time__\tztime\n__rime_date__\tzdate\n"
)


class SecurityAttributes(c.Structure):
    _fields_ = [("size", w.DWORD), ("descriptor", PTR), ("inherit", w.BOOL)]


class SecurityCapabilities(c.Structure):
    _fields_ = [("sid", PTR), ("capabilities", PTR), ("count", w.DWORD), ("reserved", w.DWORD)]


class SidAndAttributes(c.Structure):
    _fields_ = [("sid", PTR), ("attributes", w.DWORD)]


class StartupInfo(c.Structure):
    _fields_ = [("size", w.DWORD), ("reserved", w.LPWSTR), ("desktop", w.LPWSTR), ("title", w.LPWSTR),
                ("x", w.DWORD), ("y", w.DWORD), ("xsize", w.DWORD), ("ysize", w.DWORD),
                ("xcount", w.DWORD), ("ycount", w.DWORD), ("fill", w.DWORD), ("flags", w.DWORD),
                ("show", w.WORD), ("reserved2len", w.WORD), ("reserved2", PTR),
                ("stdin", PTR), ("stdout", PTR), ("stderr", PTR)]


class StartupInfoEx(c.Structure):
    _fields_ = [("base", StartupInfo), ("attributes", PTR)]


class ProcessInformation(c.Structure):
    _fields_ = [("process", PTR), ("thread", PTR), ("pid", w.DWORD), ("tid", w.DWORD)]


def configure_api():
    kernel = c.WinDLL("kernel32", use_last_error=True)
    userenv = c.WinDLL("userenv", use_last_error=True)
    advapi = c.WinDLL("advapi32", use_last_error=True)
    userenv.CreateAppContainerProfile.argtypes = [w.LPCWSTR, w.LPCWSTR, w.LPCWSTR, PTR, w.DWORD, c.POINTER(PTR)]
    userenv.CreateAppContainerProfile.restype = c.c_long
    userenv.DeleteAppContainerProfile.argtypes = [w.LPCWSTR]
    userenv.DeleteAppContainerProfile.restype = c.c_long
    advapi.FreeSid.argtypes = [PTR]
    advapi.ConvertSidToStringSidW.argtypes = [PTR, c.POINTER(PTR)]
    kernel.CreatePipe.argtypes = [c.POINTER(PTR), c.POINTER(PTR), c.POINTER(SecurityAttributes), w.DWORD]
    kernel.SetHandleInformation.argtypes = [PTR, w.DWORD, w.DWORD]
    kernel.InitializeProcThreadAttributeList.argtypes = [PTR, w.DWORD, w.DWORD, c.POINTER(c.c_size_t)]
    kernel.UpdateProcThreadAttribute.argtypes = [PTR, w.DWORD, c.c_size_t, PTR, c.c_size_t, PTR, PTR]
    kernel.DeleteProcThreadAttributeList.argtypes = [PTR]
    kernel.CreateProcessW.argtypes = [w.LPCWSTR, w.LPWSTR, PTR, PTR, w.BOOL, w.DWORD, PTR, w.LPCWSTR,
                                      c.POINTER(StartupInfo), c.POINTER(ProcessInformation)]
    kernel.WaitForSingleObject.argtypes = [PTR, w.DWORD]
    kernel.GetExitCodeProcess.argtypes = [PTR, c.POINTER(w.DWORD)]
    kernel.PeekNamedPipe.argtypes = [PTR, PTR, w.DWORD, c.POINTER(w.DWORD), c.POINTER(w.DWORD), c.POINTER(w.DWORD)]
    kernel.ReadFile.argtypes = [PTR, PTR, w.DWORD, c.POINTER(w.DWORD), PTR]
    kernel.WriteFile.argtypes = [PTR, PTR, w.DWORD, c.POINTER(w.DWORD), PTR]
    kernel.TerminateProcess.argtypes = [PTR, w.UINT]
    kernel.CloseHandle.argtypes = [PTR]
    kernel.LocalFree.argtypes = [PTR]
    kernel.LocalFree.restype = PTR
    kernel.ProcessIdToSessionId.argtypes = [w.DWORD, c.POINTER(w.DWORD)]
    return kernel, userenv, advapi


def lpac_capabilities(kernel):
    kernelbase = c.WinDLL("kernelbase", use_last_error=True)
    kernelbase.DeriveCapabilitySidsFromName.argtypes = [w.LPCWSTR, c.POINTER(c.POINTER(PTR)), c.POINTER(w.DWORD), c.POINTER(c.POINTER(PTR)), c.POINTER(w.DWORD)]
    result = []
    for name in ("registryRead", "lpacCom", "lpacAppExperience"):
        groups, caps = c.POINTER(PTR)(), c.POINTER(PTR)()
        group_count, cap_count = w.DWORD(), w.DWORD()
        if not kernelbase.DeriveCapabilitySidsFromName(name, c.byref(groups), c.byref(group_count), c.byref(caps), c.byref(cap_count)):
            raise c.WinError(c.get_last_error())
        for index in range(group_count.value):
            kernel.LocalFree(groups[index])
        kernel.LocalFree(c.cast(groups, PTR))
        result.extend(caps[index] for index in range(cap_count.value))
        kernel.LocalFree(c.cast(caps, PTR))
    return result


def prepare_runtime(destination, source):
    source = Path(source)
    destination.mkdir()
    for name in ("python.exe", "python3.dll", "vcruntime140.dll", "vcruntime140_1.dll"):
        path = source / name
        if path.is_file():
            shutil.copy2(path, destination / name)
    for path in source.glob("python3[0-9][0-9].dll"):
        shutil.copy2(path, destination / path.name)
    shutil.copytree(source / "Lib", destination / "Lib",
                    ignore=shutil.ignore_patterns("site-packages", "__pycache__", "test", "tests", "idlelib", "tkinter", "venv", "ensurepip"))
    shutil.copytree(source / "DLLs", destination / "DLLs", ignore=shutil.ignore_patterns("__pycache__"))


def grant_read(path, directory=False):
    access = "(OI)(CI)(RX)" if directory else "(RX)"
    subprocess.run(["icacls", str(path), "/grant", "*S-1-15-2-1:" + access, "*S-1-15-2-2:" + access], check=True, capture_output=True)


def get_pipe_name(kernel, root):
    user = subprocess.run(["whoami", "/user", "/fo", "csv", "/nh"], check=True, capture_output=True, text=True).stdout
    sid = next(csv.reader([user]))[1]
    session = w.DWORD()
    if not kernel.ProcessIdToSessionId(os.getpid(), c.byref(session)):
        raise c.WinError(c.get_last_error())
    path = str(root).lower()
    if path.startswith("\\\\?\\"):
        path = path[4:]
    value = 0xcbf29ce484222325
    for byte in path.encode("utf-8"):
        value = ((value ^ byte) * 0x100000001b3) & 0xffffffffffffffff
    return f"\\\\.\\pipe\\rust-ime-v1-{sid}-{session.value}-{value:016x}"


def verify_foreign_snapshot_denied(pipe_name, token):
    for operation in (1, 6):
        for _ in range(30):
            try:
                stream = open(pipe_name, "r+b", buffering=0)
                break
            except OSError as error:
                if error.winerror != 231:
                    raise
                time.sleep(0.01)
        else:
            raise RuntimeError("測試管道忙碌")
        with stream:
            stream.write(struct.pack("<4sBBH", b"RI01", 0, operation, 12) + struct.pack("<QI", token, 0))
            try:
                result = stream.read(16)
            except OSError as error:
                if error.winerror not in (109, 232, 233):
                    raise
                result = b""
            if result:
                raise RuntimeError("不同程序不應能取得其他程序的候選快照或其反查字碼")


def sid_text(kernel, sid):
    advapi = c.WinDLL("advapi32", use_last_error=True)
    advapi.ConvertSidToStringSidW.argtypes = [PTR, c.POINTER(PTR)]
    value = PTR()
    if not advapi.ConvertSidToStringSidW(sid, c.byref(value)):
        raise c.WinError(c.get_last_error())
    try:
        return c.wstring_at(value)
    finally:
        kernel.LocalFree(value)


def set_permission(broker, setting, sid):
    subprocess.run([str(broker), setting, sid], creationflags=subprocess.CREATE_NO_WINDOW, check=True, timeout=5)


def run_child(kernel, sid, executable, script, dll, private, pipe_name, fixture, lpac, registered, server_ref, broker, desktop_dll):
    security = SecurityAttributes(c.sizeof(SecurityAttributes), None, 1)
    input_read, input_write, output_read, output_write = PTR(), PTR(), PTR(), PTR()
    handles = [input_read, input_write, output_read, output_write]
    if not kernel.CreatePipe(c.byref(input_read), c.byref(input_write), c.byref(security), 0):
        raise c.WinError(c.get_last_error())
    if not kernel.CreatePipe(c.byref(output_read), c.byref(output_write), c.byref(security), 0):
        raise c.WinError(c.get_last_error())
    if not kernel.SetHandleInformation(input_write, 1, 0) or not kernel.SetHandleInformation(output_read, 1, 0):
        raise c.WinError(c.get_last_error())
    size = c.c_size_t()
    attribute_count = 2 if lpac else 1
    kernel.InitializeProcThreadAttributeList(None, attribute_count, 0, c.byref(size))
    attributes = c.create_string_buffer(size.value)
    if not kernel.InitializeProcThreadAttributeList(attributes, attribute_count, 0, c.byref(size)):
        raise c.WinError(c.get_last_error())
    process = ProcessInformation()
    capability_sids = []
    try:
        capability_sids = lpac_capabilities(kernel) if lpac else []
        capability_array = (SidAndAttributes * len(capability_sids))(
            *(SidAndAttributes(value, 4) for value in capability_sids))
        capabilities = SecurityCapabilities(sid, c.cast(capability_array, PTR) if capability_sids else None, len(capability_sids), 0)
        if not kernel.UpdateProcThreadAttribute(attributes, 0, 0x00020009, c.byref(capabilities), c.sizeof(capabilities), None, None):
            raise c.WinError(c.get_last_error())
        if lpac:
            policy = w.DWORD(1)
            if not kernel.UpdateProcThreadAttribute(attributes, 0, 0x0002000f, c.byref(policy), c.sizeof(policy), None, None):
                raise c.WinError(c.get_last_error())
        startup = StartupInfoEx()
        startup.base.size = c.sizeof(startup)
        startup.base.flags = 0x100
        startup.base.stdin = input_read
        startup.base.stdout = output_write
        startup.base.stderr = output_write
        startup.attributes = c.cast(attributes, PTR)
        arguments = [str(executable), "-u", str(script), str(dll), str(private), pipe_name, "lpac" if lpac else "appcontainer", str(fixture / "ordinary-only.txt")]
        if registered:
            arguments.append(str(registered))
        command = c.create_unicode_buffer(subprocess.list2cmdline(arguments))
        if not kernel.CreateProcessW(str(executable), command, None, None, True, 0x00080000 | 0x08000000,
                                     None, str(fixture), c.byref(startup.base), c.byref(process)):
            raise c.WinError(c.get_last_error())
        kernel.CloseHandle(input_read)
        input_read.value = None
        kernel.CloseHandle(output_write)
        output_write.value = None
        output = bytearray()
        reloaded = False
        snapshot_checked = False
        offline_checked = False
        restarted = False
        clipboard_checked = False
        shared_checked = False
        sent = set()
        deadline = time.monotonic() + 45
        while time.monotonic() < deadline:
            available = w.DWORD()
            if kernel.PeekNamedPipe(output_read, None, 0, None, c.byref(available), None) and available.value:
                size = min(available.value, 8192)
                data, received = c.create_string_buffer(size), w.DWORD()
                if not kernel.ReadFile(output_read, data, size, c.byref(received), None):
                    raise c.WinError(c.get_last_error())
                output.extend(data.raw[:received.value])
                if len(output) > 65536:
                    raise RuntimeError("AppContainer 測試輸出過多")
            if b"READY_INITIAL_GRANT" in output and b"INITIAL_GRANT" not in sent:
                set_permission(broker, "--test-allow", sid_text(kernel, sid) + "1")
                reply = b"other allowed\n"
                written = w.DWORD()
                if not kernel.WriteFile(input_write, c.create_string_buffer(reply), len(reply), c.byref(written), None):
                    raise c.WinError(c.get_last_error())
                sent.add(b"INITIAL_GRANT")
            if not shared_checked and b"READY_SHARED" in output:
                desktop = [sys.executable, str(ROOT / "tests/tsf_uiless_activation_test.py"),
                           str(desktop_dll), "--test-fixture", "--test-shared"]
                subprocess.run(desktop, check=True, timeout=12)
                reply = b"shared\n"
                written = w.DWORD()
                if not kernel.WriteFile(input_write, c.create_string_buffer(reply), len(reply), c.byref(written), None):
                    raise c.WinError(c.get_last_error())
                shared_checked = True
            if not reloaded and b"READY_FOR_RELOAD" in output:
                private.write_text(
                    "---\n...\n即時更新詞條\tz\n跨方案測試詞\\n第二行\tmynote\n" + COMMAND_MARKERS,
                    encoding="utf-8",
                )
                data = c.create_string_buffer(b"continue\n")
                written = w.DWORD()
                if not kernel.WriteFile(input_write, data, 9, c.byref(written), None) or written.value != 9:
                    raise c.WinError(c.get_last_error())
                reloaded = True
            if not snapshot_checked:
                match = re.search(rb"SNAPSHOT_TEST (\d+)", output)
                if match:
                    verify_foreign_snapshot_denied(pipe_name, int(match.group(1)))
                    data = c.create_string_buffer(b"snapshot checked\n")
                    written = w.DWORD()
                    if not kernel.WriteFile(input_write, data, 17, c.byref(written), None) or written.value != 17:
                        raise c.WinError(c.get_last_error())
                    snapshot_checked = True
            if not offline_checked and b"READY_OFFLINE" in output:
                server_ref[0].terminate()
                server_ref[0].wait(timeout=5)
                data = c.create_string_buffer(b"offline\n")
                written = w.DWORD()
                if not kernel.WriteFile(input_write, data, 8, c.byref(written), None) or written.value != 8:
                    raise c.WinError(c.get_last_error())
                offline_checked = True
            if offline_checked and not restarted and b"READY_RESTART" in output:
                set_permission(broker, "--test-deny", sid_text(kernel, sid))
                server_ref[0] = subprocess.Popen([str(broker)], creationflags=subprocess.CREATE_NO_WINDOW, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
                time.sleep(0.5)
                if server_ref[0].poll() is not None:
                    raise RuntimeError("測試字庫程序重啟失敗")
                data = c.create_string_buffer(b"restarted\n")
                written = w.DWORD()
                if not kernel.WriteFile(input_write, data, 10, c.byref(written), None) or written.value != 10:
                    raise c.WinError(c.get_last_error())
                restarted = True
            if restarted and not clipboard_checked and b"READY_CLIPBOARD" in output:
                set_permission(broker, "--test-allow", sid_text(kernel, sid))
                reply = b"clipboard\n"
                written = w.DWORD()
                if not kernel.WriteFile(input_write, c.create_string_buffer(reply), len(reply), c.byref(written), None):
                    raise c.WinError(c.get_last_error())
                clipboard_checked = True
            if kernel.WaitForSingleObject(process.process, 0) == 0:
                break
            time.sleep(0.01)
        else:
            print(output.decode("utf-8", errors="replace"), end="")
            raise TimeoutError("AppContainer 測試逾時")
        while True:
            available = w.DWORD()
            if not kernel.PeekNamedPipe(output_read, None, 0, None, c.byref(available), None) or not available.value:
                break
            data, received = c.create_string_buffer(available.value), w.DWORD()
            if not kernel.ReadFile(output_read, data, available.value, c.byref(received), None):
                break
            output.extend(data.raw[:received.value])
        status = w.DWORD()
        kernel.GetExitCodeProcess(process.process, c.byref(status))
        text = output.decode("utf-8", errors="replace")
        print(re.sub(r"SNAPSHOT_TEST \d+", "候選快照已驗證程序隔離", text), end="")
        if status.value != 0 or not all((reloaded, snapshot_checked, offline_checked, restarted, clipboard_checked, shared_checked)) or "PASS: AppContainer TSF" not in text:
            raise RuntimeError(f"AppContainer 測試失敗，結束碼 {status.value}")
    finally:
        if process.process:
            if kernel.WaitForSingleObject(process.process, 0) != 0:
                kernel.TerminateProcess(process.process, 1)
                kernel.WaitForSingleObject(process.process, 5000)
            kernel.CloseHandle(process.process)
        if process.thread:
            kernel.CloseHandle(process.thread)
        kernel.DeleteProcThreadAttributeList(attributes)
        for value in capability_sids:
            kernel.LocalFree(value)
        for handle in handles:
            if handle:
                kernel.CloseHandle(handle)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("dll", nargs="?", default=ROOT / "work/target/release/oxide_ime.dll")
    parser.add_argument("broker", nargs="?", default=ROOT / "work/target/release/oxide-ime-broker.exe")
    parser.add_argument("--lpac", action="store_true")
    parser.add_argument("--python-root", type=Path, default=Path(sys.base_prefix))
    parser.add_argument("--registered-dll", type=Path)
    parser.add_argument("--desktop-dll", type=Path, help="可指定另一架構的 DLL 驗證同一登入工作階段的共用狀態")
    args = parser.parse_args()
    dll = Path(args.dll).resolve()
    broker_source = Path(args.broker).resolve()
    if b"RustImeSyntheticClipboardFixture" not in dll.read_bytes():
        raise RuntimeError("此測試必須使用啟用 integration-fixtures 的測試 DLL，避免讀取系統剪貼簿")
    kernel, userenv, advapi = configure_api()
    name = "RustImeTsfTest-" + uuid.uuid4().hex
    sid = PTR()
    hr = userenv.CreateAppContainerProfile(name, name, "Temporary synthetic Oxide IME TSF test", None, 0, c.byref(sid))
    if hr < 0:
        raise OSError(hr & 0xffffffff, "CreateAppContainerProfile")
    try:
        with tempfile.TemporaryDirectory(prefix="appcontainer-tsf-", dir=ROOT / "work") as tmp:
            fixture = Path(tmp).resolve()
            outputs, data, tests, runtime = [fixture / name for name in ("outputs", "data", "tests", "runtime")]
            data /= "dictionaries"
            for path in (outputs, data, tests):
                path.mkdir(parents=True)
            prepare_runtime(runtime, args.python_root)
            for name in ("appcontainer_tsf_child.py", "tsf_real_store_test.py"):
                shutil.copy2(ROOT / "tests" / name, tests / name)
            test_dll = outputs / "rust_ime_sandbox_fixture.dll"
            broker = outputs / "rust-ime-broker.exe"
            shutil.copy2(dll, test_dll)
            desktop_dll = test_dll
            if args.desktop_dll:
                desktop_dll = outputs / "rust_ime_desktop_fixture.dll"
                shutil.copy2(args.desktop_dll, desktop_dll)
            shutil.copy2(broker_source, broker)
            (data / "cangjie3.dict.yaml").write_text("---\n...\n日\ta\n曰\ta\n昌\taa\n髮\ts\n𠀀\tsx\n你\tonf\n", encoding="utf-8")
            shutil.copy2(ROOT / "assets/templates/stroke.dict.yaml", data)
            # 固定粵拼反查及同音搜尋的候選，測試只依賴這份隔離資料。
            (data / "jyut6ping3.chars.dict.csv.json").write_text(
                '{"日":{"jat6":"預設"},"逸":{"jat6":"預設"},"我":{"ngo5":"預設"}}', encoding="utf-8"
            )
            private = data / "custom.dict.yaml"
            private.write_text("---\n...\n這是一條長文測試詞條超過二十個字元測試完整輸出" + "長" * 100 + "\\n第二行\tz\n" + COMMAND_MARKERS, encoding="utf-8")
            conversion = data / "s2t"
            conversion.mkdir()
            (conversion / "custom.json").write_text('{"简体":"繁體"}', encoding="utf-8")
            (conversion / "s2t-char.min.json").write_text('{"简":"簡","体":"體"}', encoding="utf-8")
            (conversion / "s2t-phrase.min.json").write_text("{}", encoding="utf-8")
            ordinary_only = fixture / "ordinary-only.txt"
            ordinary_only.write_text("synthetic", encoding="utf-8")
            subprocess.run(["icacls", str(ordinary_only), "/grant", "*S-1-15-2-1:(RX)"], check=True, capture_output=True)
            grant_read(runtime, True)
            grant_read(tests, True)
            grant_read(test_dll)
            server_ref = [subprocess.Popen([str(broker)], creationflags=subprocess.CREATE_NO_WINDOW, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)]
            registry_key = "Software\\RustIme\\" + get_pipe_name(kernel, fixture).rsplit("-", 1)[-1]
            try:
                time.sleep(0.5)
                if server_ref[0].poll() is not None:
                    raise RuntimeError(f"測試字庫程序啟動失敗：{server_ref[0].returncode}")
                run_child(kernel, sid, runtime / "python.exe", tests / "appcontainer_tsf_child.py", test_dll, private, get_pipe_name(kernel, fixture), fixture, args.lpac, args.registered_dll, server_ref, broker, desktop_dll)
            finally:
                if server_ref[0].poll() is None:
                    server_ref[0].terminate()
                    server_ref[0].wait(timeout=5)
                logons = registry_key + r"\Logons"
                try:
                    with winreg.OpenKey(winreg.HKEY_CURRENT_USER, logons) as key:
                        names = [winreg.EnumKey(key, i) for i in range(winreg.QueryInfoKey(key)[0])]
                    for name in names:
                        winreg.DeleteKey(winreg.HKEY_CURRENT_USER, logons + "\\" + name)
                except FileNotFoundError:
                    pass
                for key in (registry_key + r"\SandboxDictionaries", logons, registry_key):
                    try:
                        winreg.DeleteKey(winreg.HKEY_CURRENT_USER, key)
                    except FileNotFoundError:
                        pass
    finally:
        advapi.FreeSid(sid)
        if userenv.DeleteAppContainerProfile(name) != 0:
            raise RuntimeError("未能移除臨時 AppContainer 測試設定")


if __name__ == "__main__":
    main()
