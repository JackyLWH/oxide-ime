// 以程序內登錄覆寫測試 32 位元 DLL，不變更真正的 HKLM 或 TSF 設定。
#include <windows.h>
#include <cstdio>
#include <string>

using Entry = HRESULT(STDAPICALLTYPE*)();
constexpr auto server_key = L"SOFTWARE\\Classes\\CLSID\\{4BD0700E-5DD1-4310-A9D2-F3D579D54730}\\InprocServer32";
constexpr auto tip_key = L"SOFTWARE\\Microsoft\\CTF\\TIP\\{4BD0700E-5DD1-4310-A9D2-F3D579D54730}";

struct TestRegistry {
    std::wstring path;
    HKEY key = nullptr;
    bool overridden = false;
    bool created = false;
    ~TestRegistry() {
        if (overridden) RegOverridePredefKey(HKEY_LOCAL_MACHINE, nullptr);
        if (key) RegCloseKey(key);
        if (created) RegDeleteTreeW(HKEY_CURRENT_USER, path.c_str());
    }
};

int wmain(int argc, wchar_t** argv) {
    if (argc != 2) return 2;
    TestRegistry registry;
    registry.path = L"Software\\RustImeRegistryTests\\x86-" + std::to_wstring(GetCurrentProcessId()) + L"-" + std::to_wstring(GetTickCount64());
    DWORD disposition = 0;
    if (RegCreateKeyExW(HKEY_CURRENT_USER, registry.path.c_str(), 0, nullptr, 0,
                        KEY_ALL_ACCESS | KEY_WOW64_32KEY, nullptr, &registry.key, &disposition) != ERROR_SUCCESS ||
        disposition != REG_CREATED_NEW_KEY) return 3;
    registry.created = true;
    if (RegOverridePredefKey(HKEY_LOCAL_MACHINE, registry.key) != ERROR_SUCCESS) return 4;
    registry.overridden = true;
    HMODULE dll = LoadLibraryW(argv[1]);
    if (!dll) return 5;
    auto reg = reinterpret_cast<Entry>(GetProcAddress(dll, "DllRegisterServer"));
    auto unreg = reinterpret_cast<Entry>(GetProcAddress(dll, "DllUnregisterServer"));
    if (!reg || !unreg || FAILED(reg())) return 6;
    wchar_t value[32768]{};
    DWORD size = sizeof(value);
    if (RegGetValueW(HKEY_LOCAL_MACHINE, server_key, nullptr, RRF_RT_REG_SZ | RRF_SUBKEY_WOW6432KEY,
                     nullptr, value, &size) != ERROR_SUCCESS || _wcsicmp(value, argv[1])) return 7;
    HKEY tip = nullptr;
    if (RegOpenKeyExW(HKEY_LOCAL_MACHINE, tip_key, 0, KEY_READ | KEY_WOW64_32KEY, &tip) == ERROR_SUCCESS) {
        RegCloseKey(tip);
        return 8;
    }
    if (FAILED(unreg())) return 9;
    HKEY deleted = nullptr;
    if (RegOpenKeyExW(HKEY_LOCAL_MACHINE, server_key, 0, KEY_READ | KEY_WOW64_32KEY, &deleted) == ERROR_SUCCESS) {
        RegCloseKey(deleted);
        return 10;
    }
    if (FAILED(reg()) || FAILED(unreg())) return 11;
    FreeLibrary(dll);
    std::puts("PASS: x86 註冊及移除只影響獨立的 32 位元 COM 類別。");
    return 0;
}
