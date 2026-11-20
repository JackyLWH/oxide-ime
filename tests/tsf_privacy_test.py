"""驗證建立 TSF 測試資料時不讀取或複製使用者的自訂字典。"""
import builtins
import io
import os
from pathlib import Path
import shutil
import sys
from unittest import mock

sys.dont_write_bytecode = True
import tsf_real_store_test as integration

root = Path(__file__).resolve().parent.parent
private_dictionary = (root / "data" / "dictionaries" / "custom.dict.yaml").resolve()
original_builtin_open = builtins.open
original_io_open = io.open
original_copy = shutil.copy2


def check_path(path):
    if isinstance(path, (str, bytes, os.PathLike)) and Path(os.fsdecode(path)).resolve() == private_dictionary:
        raise RuntimeError("測試不應讀取或複製使用者的自訂字典")


def guarded_builtin_open(path, *args, **kwargs):
    check_path(path)
    return original_builtin_open(path, *args, **kwargs)


def guarded_io_open(path, *args, **kwargs):
    check_path(path)
    return original_io_open(path, *args, **kwargs)


def guarded_copy(source, destination, *args, **kwargs):
    check_path(source)
    return original_copy(source, destination, *args, **kwargs)


def main():
    dll = Path(sys.argv[1] if len(sys.argv) > 1 else integration.release_dll()).resolve()
    sys.argv = [str(Path(integration.__file__).resolve()), str(dll)]
    with mock.patch.object(builtins, "open", guarded_builtin_open), \
         mock.patch.object(io, "open", guarded_io_open), \
         mock.patch.object(shutil, "copy2", guarded_copy):
        integration.main()
    print("PASS: 建立測試資料時未讀取或複製使用者的自訂字典。", flush=True)


if __name__ == "__main__":
    main()
