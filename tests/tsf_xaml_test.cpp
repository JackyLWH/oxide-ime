// 測試宿主：在獨立桌面的 XAML 文字框驗證沉浸式 TSF，不操作使用者視窗。
#include <windows.h>
#undef GetCurrentTime
#include <msctf.h>
#include <windows.ui.xaml.hosting.desktopwindowxamlsource.h>
#include <winrt/Windows.Foundation.h>
#include <winrt/Windows.UI.Xaml.Controls.h>
#include <winrt/Windows.UI.Xaml.Hosting.h>
#include <algorithm>
#include <cstdio>
#include <filesystem>
#include <fstream>
#include <iterator>
#include <stdexcept>
#include <string>

using namespace winrt;
using namespace Windows::UI::Xaml;
using namespace Windows::UI::Xaml::Controls;
using namespace Windows::UI::Xaml::Hosting;

constexpr GUID service_id{0x4bd0700e, 0x5dd1, 0x4310, {0xa9, 0xd2, 0xf3, 0xd5, 0x79, 0xd5, 0x47, 0x30}};
constexpr GUID profile_id{0x21a75529, 0x20a1, 0x4cf2, {0xb7, 0xd8, 0x1b, 0xb9, 0x42, 0x67, 0xe1, 0xc9}};

void check(char const* operation, HRESULT hr) {
    if (FAILED(hr)) {
        std::printf("%s: 0x%08lX\n", operation, static_cast<unsigned long>(hr));
        throw hresult_error(hr);
    }
}

void pump(unsigned milliseconds) {
    auto const until = GetTickCount64() + milliseconds;
    while (GetTickCount64() < until) {
        MSG message{};
        while (PeekMessageW(&message, nullptr, 0, 0, PM_REMOVE)) {
            TranslateMessage(&message);
            DispatchMessageW(&message);
        }
        Sleep(10);
    }
}

// 測試程序中的代理保留 TSF 實際啟用的服務，以測試 XAML 的真實文字儲存。
struct FactoryProxy final : IClassFactory {
    LONG refs = 1;
    com_ptr<IClassFactory> inner;
    com_ptr<ITfTextInputProcessor> processor;

    HRESULT STDMETHODCALLTYPE QueryInterface(REFIID iid, void** out) override {
        if (!out) return E_POINTER;
        *out = nullptr;
        if (iid != IID_IUnknown && iid != IID_IClassFactory) return E_NOINTERFACE;
        *out = static_cast<IClassFactory*>(this);
        AddRef();
        return S_OK;
    }
    ULONG STDMETHODCALLTYPE AddRef() override { return InterlockedIncrement(&refs); }
    ULONG STDMETHODCALLTYPE Release() override {
        ULONG count = InterlockedDecrement(&refs);
        if (!count) delete this;
        return count;
    }
    HRESULT STDMETHODCALLTYPE CreateInstance(IUnknown* outer, REFIID iid, void** out) override {
        HRESULT hr = inner->CreateInstance(outer, iid, out);
        if (SUCCEEDED(hr) && out && *out) {
            com_ptr<ITfTextInputProcessor> found;
            if (SUCCEEDED(static_cast<IUnknown*>(*out)->QueryInterface(IID_PPV_ARGS(found.put())))) {
                processor = std::move(found);
            }
        }
        return hr;
    }
    HRESULT STDMETHODCALLTYPE LockServer(BOOL lock) override { return inner->LockServer(lock); }
};

struct ClassRegistration {
    DWORD cookie = 0;
    ~ClassRegistration() { if (cookie) CoRevokeClassObject(cookie); }
};

void press(ITfKeyEventSink* sink, ITfContext* context, WPARAM key) {
    BOOL eaten = FALSE;
    check("OnTestKeyDown", sink->OnTestKeyDown(context, key, 0, &eaten));
    if (!eaten) throw std::runtime_error("輸入法未接受測試按鍵");
    check("OnKeyDown", sink->OnKeyDown(context, key, 0, &eaten));
    if (!eaten) throw std::runtime_error("輸入法未處理測試按鍵");
    check("OnTestKeyUp", sink->OnTestKeyUp(context, key, 0, &eaten));
    if (eaten) check("OnKeyUp", sink->OnKeyUp(context, key, 0, &eaten));
}

std::wstring normalise_lines(std::wstring text) {
    for (auto position = text.find(L"\r\n"); position != std::wstring::npos;
            position = text.find(L"\r\n", position)) {
        text.erase(position, 1);
    }
    std::replace(text.begin(), text.end(), L'\r', L'\n');
    return text;
}

void check_long_text(TextBox const& box, ITfKeyEventSink* sink, ITfContext* context,
                     wchar_t const* path) {
    std::ifstream file(std::filesystem::path(path), std::ios::binary);
    if (!file) throw std::runtime_error("無法讀取多行測試詞條");
    std::string bytes(std::istreambuf_iterator<char>{file}, std::istreambuf_iterator<char>{});
    if (bytes.starts_with("\xef\xbb\xbf")) bytes.erase(0, 3);
    auto candidate = normalise_lines(to_hstring(bytes).c_str());
    while (!candidate.empty() && candidate.back() == L'\n') candidate.pop_back();
    const std::wstring prefix = L"開頭🙂。";
    const std::wstring suffix = L"甲乙丙丁戊己庚辛壬癸原有後文。";
    for (auto const selection : {L"", L"取代此處"}) {
        box.Text(prefix + selection + suffix);
        box.Select(static_cast<int>(prefix.size()), static_cast<int>(wcslen(selection)));
        pump(100);
        press(sink, context, 'Z');
        press(sink, context, 'S');
        press(sink, context, '1');
        pump(150);
        auto text = std::wstring(box.Text().c_str());
        const auto caret = static_cast<size_t>(box.SelectionStart());
        const bool exact = normalise_lines(text) == prefix + candidate + suffix;
        if (!exact || box.SelectionLength() != 0 || caret > text.size() ||
            normalise_lines(text.substr(0, caret)) != prefix + candidate) {
            std::printf("zs: text=%d caret=%zu expected-normalised=%zu\n",
                        exact, caret, prefix.size() + candidate.size());
            throw std::runtime_error("多行 zs 插入點或前後文字不符");
        }
    }
    std::printf("PASS: zs %zu UTF-16 units, XAML exact text and caret for insertion and replacement.\n",
                candidate.size());
}

void run_test(bool registered, wchar_t const* dll_path, wchar_t const* long_text) {
    com_ptr<IClassFactory> proxy_owner;
    FactoryProxy* proxy = nullptr;
    ClassRegistration registration;
    if (!registered) {
        // 必須先註冊代理；若目前系統已選用 Oxide IME，建立 XAML 視窗時便可能先啟用文字服務。
        HMODULE dll = LoadLibraryW(dll_path);
        if (!dll) throw hresult_error(HRESULT_FROM_WIN32(GetLastError()));
        auto factory = reinterpret_cast<HRESULT (STDAPICALLTYPE*)(REFCLSID, REFIID, void**)>(
            GetProcAddress(dll, "DllGetClassObject"));
        if (!factory) throw hresult_error(HRESULT_FROM_WIN32(GetLastError()));
        proxy = new FactoryProxy;
        proxy_owner.attach(proxy);
        check("DllGetClassObject", factory(service_id, IID_PPV_ARGS(proxy->inner.put())));
        check("CoRegisterClassObject", CoRegisterClassObject(service_id, proxy_owner.get(), CLSCTX_INPROC_SERVER,
                                                              REGCLS_MULTIPLEUSE, &registration.cookie));
    }
    HWND window = CreateWindowExW(0, L"STATIC", L"oxide-ime XAML test", WS_OVERLAPPEDWINDOW,
                                  100, 100, 500, 250, nullptr, nullptr, GetModuleHandleW(nullptr), nullptr);
    if (!window) throw hresult_error(HRESULT_FROM_WIN32(GetLastError()));
    auto xaml = WindowsXamlManager::InitializeForCurrentThread();
    DesktopWindowXamlSource source;
    auto interop = source.as<IDesktopWindowXamlSourceNative>();
    check("AttachToWindow", interop->AttachToWindow(window));
    HWND island = nullptr;
    check("get_WindowHandle", interop->get_WindowHandle(&island));
    SetWindowPos(island, nullptr, 0, 0, 480, 200, SWP_SHOWWINDOW);
    TextBox box;
    box.AcceptsReturn(true);
    source.Content(box);
    ShowWindow(window, SW_SHOW);
    SetFocus(island);
    pump(300);
    if (!box.Focus(FocusState::Programmatic)) throw std::runtime_error("XAML 文字框無法取得焦點");
    pump(300);

    com_ptr<ITfThreadMgrEx> thread;
    check("ITfThreadMgrEx", CoCreateInstance(CLSID_TF_ThreadMgr, nullptr, CLSCTX_INPROC_SERVER,
                                              IID_PPV_ARGS(thread.put())));
    DWORD flags = 0;
    check("GetActiveFlags", thread->GetActiveFlags(&flags));
    if (!(flags & TF_TMF_IMMERSIVEMODE)) throw std::runtime_error("XAML 未進入沉浸式 TSF 模式");
    com_ptr<ITfInputProcessorProfileMgr> profiles;
    check("ITfInputProcessorProfileMgr", CoCreateInstance(CLSID_TF_InputProcessorProfiles, nullptr,
                                                             CLSCTX_INPROC_SERVER, IID_PPV_ARGS(profiles.put())));

    check("ActivateProfile", profiles->ActivateProfile(TF_PROFILETYPE_INPUTPROCESSOR, 0x0404,
                                                         service_id, profile_id, nullptr,
                                                         TF_IPPMF_FORPROCESS | TF_IPPMF_DONTCARECURRENTINPUTLANGUAGE));
    pump(300);
    auto keys = thread.as<ITfKeystrokeMgr>();
    GUID foreground{};
    check("GetForeground", keys->GetForeground(&foreground));
    if (foreground != service_id) throw std::runtime_error("沉浸式執行緒未啟用 Oxide IME");

    if (registered) {
        if (!GetModuleHandleW(dll_path)) throw std::runtime_error("沉浸式執行緒未載入已註冊的 DLL");
        constexpr GUID attribute_id{0x240e3ecc, 0xa260, 0x41cf, {0x98, 0x5a, 0xe1, 0x72, 0x52, 0x57, 0x28, 0x03}};
        com_ptr<ITfDisplayAttributeMgr> attributes;
        check("ITfDisplayAttributeMgr", CoCreateInstance(CLSID_TF_DisplayAttributeMgr, nullptr,
                                                         CLSCTX_INPROC_SERVER, IID_PPV_ARGS(attributes.put())));
        com_ptr<ITfDisplayAttributeInfo> info;
        CLSID owner{};
        check("GetDisplayAttributeInfo", attributes->GetDisplayAttributeInfo(attribute_id, info.put(), &owner));
        TF_DISPLAYATTRIBUTE style{};
        check("GetAttributeInfo", info->GetAttributeInfo(&style));
        if (owner != service_id || style.lsStyle != TF_LS_SOLID || style.fBoldLine || style.bAttr != TF_ATTR_INPUT)
            throw std::runtime_error("已註冊的組字底線屬性不正確");
        std::puts("PASS: Windows 在沉浸式 XAML 執行緒載入已註冊的輸入法。");
        std::puts("PASS: Windows 能取得已註冊的組字實線顯示屬性。");
    } else {
        com_ptr<ITfDocumentMgr> document;
        check("GetFocus", thread->GetFocus(document.put()));
        if (!document) throw std::runtime_error("XAML 文字框沒有 TSF 焦點文件");
        if (!proxy->processor) throw std::runtime_error("測試用文字服務沒有由代理建立");
        com_ptr<ITfContext> context;
        check("GetTop", document->GetTop(context.put()));
        if (!context) throw std::runtime_error("找不到 XAML 文字內容");
        auto sink = proxy->processor.as<ITfKeyEventSink>();
        BOOL eaten = FALSE;
        check("Shift down", sink->OnTestKeyDown(context.get(), VK_SHIFT, 0, &eaten));
        if (eaten) throw std::runtime_error("Shift 按下不應被吞掉");
        check("Shift up test", sink->OnTestKeyUp(context.get(), VK_SHIFT, 0, &eaten));
        if (!eaten) throw std::runtime_error("Shift 未切換輸入模式");
        check("Shift up", sink->OnKeyUp(context.get(), VK_SHIFT, 0, &eaten));
        press(sink.get(), context.get(), 'A');
        pump(100);
        if (box.Text() != L"\u65e5") throw std::runtime_error("XAML 預編輯文字不正確");
        press(sink.get(), context.get(), '2');
        pump(100);
        if (box.Text() != L"\u66f0") throw std::runtime_error("XAML 第二候選未正確上屏");
        press(sink.get(), context.get(), 'A');
        press(sink.get(), context.get(), VK_ESCAPE);
        pump(100);
        if (box.Text() != L"\u66f0") throw std::runtime_error("XAML 取消組字未保留已提交文字");
        std::puts("PASS: XAML 文字框的中文組字、選字及取消正確。");
        press(sink.get(), context.get(), 'Z');
        press(sink.get(), context.get(), '1');
        pump(100);
        // XAML TextBox 可以把 TSF 的 LF 正規化為 CR 或 CRLF。
        auto text = normalise_lines(box.Text().c_str());
        if (text != L"曰第一行。\n第二行🙂。\n最後一行。") {
            std::printf("XAML UTF-16 (%zu):", text.size());
            for (wchar_t unit : text) std::printf(" %04X", static_cast<unsigned>(unit));
            std::puts("");
            std::fflush(stdout);
            throw std::runtime_error("XAML 多行提交未保留標點、補充平面字元或換行");
        }
        std::puts("PASS: XAML 保留多行候選文字的全部標點及換行。");
        if (long_text) check_long_text(box, sink.get(), context.get(), long_text);

        box.Text(L"");
        box.Select(0, 0);
        pump(100);
        BYTE keyboard_state[256]{};
        if (!GetKeyboardState(keyboard_state)) throw hresult_error(HRESULT_FROM_WIN32(GetLastError()));
        auto ctrl_slash = [&] {
            BYTE modified[256]{};
            std::copy_n(keyboard_state, 256, modified);
            modified[VK_CONTROL] |= 0x80;
            if (!SetKeyboardState(modified)) throw hresult_error(HRESULT_FROM_WIN32(GetLastError()));
            try {
                press(sink.get(), context.get(), VK_OEM_2);
            } catch (...) {
                SetKeyboardState(keyboard_state);
                throw;
            }
            if (!SetKeyboardState(keyboard_state)) throw hresult_error(HRESULT_FROM_WIN32(GetLastError()));
        };
        ctrl_slash();
        press(sink.get(), context.get(), 'S');
        press(sink.get(), context.get(), '1');
        pump(100);
        if (box.Text() != L"測") throw std::runtime_error("XAML 未從新目錄載入自訂繁轉簡詞表");
        ctrl_slash();
        press(sink.get(), context.get(), VK_OEM_3);
        for (auto key : {'N', 'G', 'O', 'X', 'X'}) press(sink.get(), context.get(), key);
        press(sink.get(), context.get(), '1');
        pump(100);
        if (box.Text() != L"測測") throw std::runtime_error("XAML 未按新 JSON 原有順序選取粵拼同級候選");
        std::puts("PASS: XAML loads t2s files and ranks Jyutping using the installed JSON order.");
    }
    std::fflush(stdout);
    source.Close();
    xaml.Close();
    DestroyWindow(window);
}

int wmain(int argc, wchar_t** argv) {
    if ((argc != 3 && argc != 4) ||
        (wcscmp(argv[1], L"--registered") && wcscmp(argv[1], L"--fixture"))) {
        std::fputs("Usage: tsf_xaml_test.exe --registered|--fixture DLL [long-text.txt]\n", stderr);
        return 1;
    }
    auto desktop_name = std::wstring(L"oxide-ime-xaml-test-") + std::to_wstring(GetCurrentProcessId());
    HDESK desktop = CreateDesktopW(desktop_name.c_str(), nullptr, nullptr, 0, GENERIC_ALL, nullptr);
    if (!desktop || !SetThreadDesktop(desktop)) {
        std::printf("Create isolated desktop: %lu\n", GetLastError());
        return 1;
    }
    int result = 0;
    try {
        init_apartment(apartment_type::single_threaded);
        run_test(wcscmp(argv[1], L"--registered") == 0, argv[2], argc == 4 ? argv[3] : nullptr);
        uninit_apartment();
    } catch (hresult_error const& error) {
        std::printf("HRESULT: 0x%08lX\n", static_cast<unsigned long>(error.code().value));
        result = 2;
    } catch (std::exception const& error) {
        std::printf("ERROR: %s\n", error.what());
        result = 2;
    }
    return result;
}
