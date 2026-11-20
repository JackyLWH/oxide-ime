#define WIN32_LEAN_AND_MEAN
#define NOMINMAX
#include <windows.h>
#include <evntrace.h>
#include <evntcons.h>
#include <fcntl.h>
#include <io.h>
#include <tlhelp32.h>

#include <algorithm>
#include <cstddef>
#include <cstdint>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <iterator>
#include <atomic>
#include <memory>
#include <mutex>
#include <string>
#include <system_error>
#include <thread>
#include <type_traits>
#include <vector>

namespace {

using Ticks = unsigned long long; // Windows FILETIME，以 100 奈秒為單位。
constexpr wchar_t session_name[] = L"OxideIme-LoL-EntryProbe-v3";
constexpr GUID session_guid = {0xbacead85, 0xf944, 0x4b01, {0xb4, 0x5b, 0xae, 0x1d, 0x56, 0xa6, 0x1d, 0x80}};
constexpr GUID image_provider = {0x2cb15d1d, 0x5fc1, 0x11d2, {0xab, 0xe1, 0x00, 0xa0, 0xc9, 0x11, 0xf5, 0x18}};
constexpr GUID fault_provider = {0x3d6fa8d3, 0xfe05, 0x11d0, {0x9d, 0xda, 0x00, 0xc0, 0x4f, 0xd7, 0xba, 0x7c}};
constexpr DWORD sample_ms = 15;
constexpr DWORD initial_flush_ms = 40;
constexpr DWORD shutdown_grace_ms = 5000;

class Handle {
public:
    explicit Handle(HANDLE value = nullptr) : value_(value) {}
    ~Handle() { reset(); }
    Handle(const Handle&) = delete;
    Handle& operator=(const Handle&) = delete;
    void reset(HANDLE value = nullptr) {
        if (valid()) CloseHandle(value_);
        value_ = value;
    }
    bool valid() const { return value_ && value_ != INVALID_HANDLE_VALUE; }
    HANDLE get() const { return value_; }

private:
    HANDLE value_;
};

Ticks current_time() {
    FILETIME time{};
    GetSystemTimePreciseAsFileTime(&time);
    return (static_cast<Ticks>(time.dwHighDateTime) << 32) | time.dwLowDateTime;
}

struct Image {
    std::wstring dos_path;
    std::wstring nt_path;
    DWORD entry = 0;
};

bool read_image(const wchar_t* path, Image& image) {
    const Handle file(CreateFileW(path, GENERIC_READ, FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                                  nullptr, OPEN_EXISTING, FILE_ATTRIBUTE_NORMAL, nullptr));
    if (!file.valid()) return false;
    DWORD count = 0;
    IMAGE_DOS_HEADER dos{};
    if (!ReadFile(file.get(), &dos, sizeof(dos), &count, nullptr) || count != sizeof(dos)
        || dos.e_magic != IMAGE_DOS_SIGNATURE || dos.e_lfanew < static_cast<LONG>(sizeof(dos))) return false;
    LARGE_INTEGER offset{};
    offset.QuadPart = dos.e_lfanew;
    IMAGE_NT_HEADERS64 pe{};
    if (!SetFilePointerEx(file.get(), offset, nullptr, FILE_BEGIN)
        || !ReadFile(file.get(), &pe, sizeof(pe), &count, nullptr) || count != sizeof(pe)
        || pe.Signature != IMAGE_NT_SIGNATURE || pe.FileHeader.Machine != IMAGE_FILE_MACHINE_AMD64
        || (pe.FileHeader.Characteristics & IMAGE_FILE_DLL) == 0
        || pe.OptionalHeader.Magic != IMAGE_NT_OPTIONAL_HDR64_MAGIC
        || pe.OptionalHeader.AddressOfEntryPoint == 0
        || pe.OptionalHeader.AddressOfEntryPoint >= pe.OptionalHeader.SizeOfImage) return false;

    wchar_t resolved[MAX_PATH]{};
    DWORD length = GetFinalPathNameByHandleW(file.get(), resolved, MAX_PATH, VOLUME_NAME_NT);
    if (!length || length >= MAX_PATH) return false;
    image.nt_path.assign(resolved, length);
    length = GetFinalPathNameByHandleW(file.get(), resolved, MAX_PATH, VOLUME_NAME_DOS);
    if (length <= 4 || length >= MAX_PATH || wcsncmp(resolved, L"\\\\?\\", 4) != 0) return false;
    image.dos_path.assign(resolved + 4, length - 4);
    image.entry = pe.OptionalHeader.AddressOfEntryPoint;
    return true;
}

bool registered_image(Image& image) {
    constexpr wchar_t key[] = L"SOFTWARE\\Classes\\CLSID\\{4BD0700E-5DD1-4310-A9D2-F3D579D54730}\\InprocServer32";
    wchar_t path[MAX_PATH]{};
    DWORD bytes = sizeof(path);
    return RegGetValueW(HKEY_LOCAL_MACHINE, key, nullptr, RRF_RT_REG_SZ | RRF_SUBKEY_WOW6464KEY,
                        nullptr, path, &bytes) == ERROR_SUCCESS
        && read_image(path, image);
}

bool is_target(DWORD pid, const wchar_t* executable) {
    DWORD own_session = 0, target_session = 0;
    if (!ProcessIdToSessionId(GetCurrentProcessId(), &own_session)
        || !ProcessIdToSessionId(pid, &target_session) || own_session != target_session) return false;
    const Handle snapshot(CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0));
    if (!snapshot.valid()) return false;
    PROCESSENTRY32W process{};
    process.dwSize = sizeof(process);
    for (BOOL found = Process32FirstW(snapshot.get(), &process); found;
         found = Process32NextW(snapshot.get(), &process)) {
        if (process.th32ProcessID == pid) return _wcsicmp(process.szExeFile, executable) == 0;
    }
    return false;
}

struct Page {
    DWORD state = 0;
    DWORD protect = 0;
    DWORD type = 0;
    DWORD error = 0;
    bool same_allocation = false;
    bool operator==(const Page&) const = default;
};

struct Samples {
    Page page{};
    Ticks first = 0;
    Ticks last = 0;
    Ticks max_gap = 0;
    unsigned count = 0;
};

struct Probe {
    const Image& image;
    const wchar_t* executable;
    HANDLE finished;
    std::mutex mutex;
    Handle process;
    std::uint64_t base = 0;
    Ticks loaded = 0;
    DWORD pid = 0;
    unsigned attempt = 0;
    Samples samples{};

    Probe(const Image& dll, const wchar_t* name, HANDLE finished_event)
        : image(dll), executable(name), finished(finished_event) {}

    void flush() {
        if (!samples.count) return;
        const auto& data = samples.page;
        std::printf("{\"kind\":\"page\",\"attempt\":%u,\"first_filetime\":\"%llu\",\"last_filetime\":\"%llu\","
                    "\"max_gap_100ns\":%llu,\"count\":%u,\"state\":%lu,\"protect\":%lu,\"type\":%lu,"
                    "\"same_allocation\":%s,\"query_error\":%lu}\n",
                    attempt, samples.first, samples.last, samples.max_gap, samples.count,
                    data.state, data.protect, data.type, data.same_allocation ? "true" : "false", data.error);
        samples = {};
    }

    void sample() {
        if (!process.valid()) return;
        MEMORY_BASIC_INFORMATION info{};
        const auto* entry = reinterpret_cast<const void*>(base + image.entry);
        Page page;
        if (VirtualQueryEx(process.get(), entry, &info, sizeof(info)) == sizeof(info)) {
            page = {info.State, info.Protect, info.Type, 0,
                    reinterpret_cast<std::uint64_t>(info.AllocationBase) == base};
        } else {
            page.error = GetLastError();
        }
        const auto now = current_time();
        if (!samples.count || samples.page != page || now < samples.last || now - samples.first >= 10'000'000) {
            flush();
            samples = {page, now, now, 0, 1};
        } else {
            samples.max_gap = std::max(samples.max_gap, now - samples.last);
            samples.last = now;
            ++samples.count;
        }
    }

    void load(DWORD loaded_pid, DWORD tid, std::uint64_t address, Ticks time) {
        if (pid) return;
        loaded = time;
        pid = loaded_pid;
        base = address;
        ++attempt;
        std::printf("{\"kind\":\"load\",\"attempt\":%u,\"pid\":%lu,\"tid\":%lu,\"filetime\":\"%llu\","
                    "\"observed_filetime\":\"%llu\"}\n", attempt, pid, tid, time, current_time());
        // 只向 Windows 查詢登錄 DLL 入口的一頁中繼資料。
        process.reset(OpenProcess(PROCESS_QUERY_INFORMATION, FALSE, pid));
        const DWORD error = process.valid() ? ERROR_SUCCESS : GetLastError();
        std::printf("{\"kind\":\"query_open\",\"attempt\":%u,\"win32\":%lu}\n", attempt, error);
        sample();
    }
};

template<typename T>
T read_at(const EVENT_RECORD& event, std::size_t offset) {
    T value{};
    std::memcpy(&value, static_cast<const std::byte*>(event.UserData) + offset, sizeof(value));
    return value;
}

bool same_path(const EVENT_RECORD& event, std::size_t offset, const std::wstring& path) {
    const auto bytes = path.size() * sizeof(wchar_t);
    if (event.UserDataLength < offset + bytes + sizeof(wchar_t)
        || read_at<wchar_t>(event, offset + bytes) != L'\0') return false;
    wchar_t name[MAX_PATH];
    std::memcpy(name, static_cast<const std::byte*>(event.UserData) + offset, bytes);
    return CompareStringOrdinal(name, static_cast<int>(path.size()), path.c_str(),
                                static_cast<int>(path.size()), TRUE) == CSTR_EQUAL;
}

void WINAPI on_event(PEVENT_RECORD record) {
    const auto& header = record->EventHeader;
    if (!record->UserData || (header.Flags & EVENT_HEADER_FLAG_64_BIT_HEADER) == 0) return;
    auto& probe = *static_cast<Probe*>(record->UserContext);
    const auto opcode = header.EventDescriptor.Opcode;
    const auto version = header.EventDescriptor.Version;
    const auto time = static_cast<Ticks>(header.TimeStamp.QuadPart);
    if (header.ProviderId == image_provider && (version == 2 || version == 3)) {
        // Image_Load v2、v3 的 64 位元檔名之前有三個指標欄位及八個 DWORD。
        constexpr std::size_t path_offset = 3 * sizeof(std::uint64_t) + 8 * sizeof(DWORD);
        if (record->UserDataLength < path_offset || (opcode != 10 && opcode != 2)) return;
        const auto pid = read_at<DWORD>(*record, 2 * sizeof(std::uint64_t));
        const auto base = read_at<std::uint64_t>(*record, 0);
        if (opcode == 10) {
            if ((same_path(*record, path_offset, probe.image.nt_path)
                 || same_path(*record, path_offset, probe.image.dos_path)) && is_target(pid, probe.executable)) {
                std::lock_guard lock(probe.mutex);
                probe.load(pid, header.ThreadId, base, time);
            }
        } else {
            std::lock_guard lock(probe.mutex);
            if (probe.pid && probe.pid == pid && probe.base == base && time >= probe.loaded) {
                probe.flush();
                std::printf("{\"kind\":\"unload\",\"attempt\":%u,\"filetime\":\"%llu\"}\n", probe.attempt, time);
                probe.process.reset();
                probe.pid = 0;
            }
        }
    } else if (header.ProviderId == fault_provider && opcode == 15
               && record->UserDataLength >= 2 * sizeof(std::uint64_t)) {
        std::lock_guard lock(probe.mutex);
        const auto entry = probe.base + probe.image.entry;
        if (probe.pid && probe.pid == header.ProcessId && time >= probe.loaded
            && read_at<std::uint64_t>(*record, 0) == entry && read_at<std::uint64_t>(*record, 8) == entry) {
            std::printf("{\"kind\":\"fault\",\"attempt\":%u,\"tid\":%lu,\"filetime\":\"%llu\"}\n",
                        probe.attempt, header.ThreadId, time);
        }
    }
}

HANDLE cancel_event = nullptr;

BOOL WINAPI cancel_capture(DWORD code) {
    if (code != CTRL_C_EVENT && code != CTRL_BREAK_EVENT) return FALSE;
    SetEvent(cancel_event);
    return TRUE;
}

struct CaptureState {
    volatile LONG64 session = 0;
};

class SharedState {
public:
    explicit SharedState(HANDLE mapping)
        : state_(static_cast<CaptureState*>(MapViewOfFile(mapping, FILE_MAP_ALL_ACCESS, 0, 0, sizeof(CaptureState)))) {}
    ~SharedState() { if (state_) UnmapViewOfFile(state_); }
    SharedState(const SharedState&) = delete;
    SharedState& operator=(const SharedState&) = delete;
    CaptureState* get() const { return state_; }

private:
    CaptureState* state_;
};

int capture(const Image& image, const wchar_t* executable, DWORD duration_ms,
            HANDLE requested_stop = nullptr, CaptureState* shared = nullptr) {
    const Handle finished(CreateEventW(nullptr, TRUE, FALSE, nullptr));
    if (!finished.valid()) return 1;
    struct {
        EVENT_TRACE_PROPERTIES properties{};
        wchar_t name[std::size(session_name)]{};
    } info;
    auto& p = info.properties;
    p.Wnode.BufferSize = sizeof(info);
    p.Wnode.Flags = WNODE_FLAG_TRACED_GUID;
    p.Wnode.ClientContext = 1;
    p.Wnode.Guid = session_guid;
    p.BufferSize = 128;
    p.FlushTimer = 1;
    p.LogFileMode = EVENT_TRACE_REAL_TIME_MODE | EVENT_TRACE_SYSTEM_LOGGER_MODE;
    p.EnableFlags = EVENT_TRACE_FLAG_IMAGE_LOAD | EVENT_TRACE_FLAG_MEMORY_PAGE_FAULTS | EVENT_TRACE_FLAG_NO_SYSCONFIG;
    p.LoggerNameOffset = sizeof(EVENT_TRACE_PROPERTIES);

    CONTROLTRACE_ID session = 0;
    const auto start = StartTraceW(&session, session_name, &p);
    if (start != ERROR_SUCCESS) {
        std::printf("{\"kind\":\"error\",\"operation\":\"StartTraceW\",\"win32\":%lu}\n", start);
        return 2;
    }
    if (shared) InterlockedExchange64(&shared->session, static_cast<LONG64>(session));
    const auto stop_session = [&] {
        std::printf("{\"kind\":\"trace_stop\"}\n");
        const auto stopped = ControlTraceW(session, nullptr, &p, EVENT_TRACE_CONTROL_STOP);
        if (shared && stopped == ERROR_SUCCESS) InterlockedExchange64(&shared->session, 0);
        return stopped;
    };
    Probe probe(image, executable, finished.get());
    EVENT_TRACE_LOGFILEW stream{};
    stream.LoggerName = info.name;
    stream.ProcessTraceMode = PROCESS_TRACE_MODE_REAL_TIME | PROCESS_TRACE_MODE_EVENT_RECORD;
    stream.EventRecordCallback = on_event;
    stream.Context = &probe;
    auto token = OpenTraceW(&stream);
    if (token == INVALID_PROCESSTRACE_HANDLE) {
        const auto error = GetLastError();
        stop_session();
        std::printf("{\"kind\":\"error\",\"operation\":\"OpenTraceW\",\"win32\":%lu}\n", error);
        return 2;
    }

    ULONG processed = ERROR_SUCCESS;
    ULONG stopped = ERROR_SUCCESS;
    ULONG flush_error = ERROR_SUCCESS;
    DWORD wait_error = ERROR_SUCCESS;
    cancel_event = finished.get();
    const BOOL handler_set = !requested_stop && SetConsoleCtrlHandler(cancel_capture, TRUE);
    try {
        std::printf("{\"kind\":\"ready\",\"format\":3,\"entry_rva\":%lu,\"sample_ms\":%lu,\"initial_flush_ms\":%lu}\n",
                    image.entry, sample_ms, initial_flush_ms);
        std::fflush(stdout);
        std::thread reader([&] {
            processed = ProcessTrace(&token, 1, nullptr, nullptr);
            SetEvent(probe.finished);
        });
        const auto expires = GetTickCount64() + duration_ms;
        auto next_flush = GetTickCount64() + initial_flush_ms;
        const HANDLE waits[] = {finished.get(), requested_stop};
        while (GetTickCount64() < expires) {
            const auto waited = WaitForMultipleObjects(requested_stop ? 2 : 1, waits, FALSE, sample_ms);
            if (waited != WAIT_TIMEOUT) {
                if (waited == WAIT_FAILED) wait_error = GetLastError();
                break;
            }
            bool awaiting_first_load = false;
            {
                std::lock_guard lock(probe.mutex);
                probe.sample();
                awaiting_first_load = probe.attempt == 0;
            }
            // 對戰程序的首次映射可能不足一秒。縮短第一筆載入事件留在 ETW 緩衝區的時間。
            if (awaiting_first_load && flush_error == ERROR_SUCCESS && GetTickCount64() >= next_flush) {
                flush_error = ControlTraceW(session, nullptr, &p, EVENT_TRACE_CONTROL_FLUSH);
                next_flush = GetTickCount64() + initial_flush_ms;
            }
        }
        // 先停止追蹤工作階段，讓消費者讀完最後一批緩衝區。
        stopped = stop_session();
        if (stopped != ERROR_SUCCESS) CloseTrace(token);
        reader.join();
        if (stopped == ERROR_SUCCESS) CloseTrace(token);
    } catch (const std::system_error&) {
        CloseTrace(token);
        stop_session();
        if (handler_set) SetConsoleCtrlHandler(cancel_capture, FALSE);
        return 2;
    }
    if (handler_set) SetConsoleCtrlHandler(cancel_capture, FALSE);
    probe.flush();
    std::printf("{\"kind\":\"capture_end\",\"attempts\":%u,\"trace_error\":%lu,\"stop_error\":%lu,"
                "\"wait_error\":%lu,\"early_flush_error\":%lu,\"events_lost\":%lu,\"buffers_lost\":%lu}\n",
                probe.attempt, processed, stopped, wait_error, flush_error, p.EventsLost, p.RealTimeBuffersLost);
    return processed == ERROR_SUCCESS && stopped == ERROR_SUCCESS && !wait_error
        && !p.EventsLost && !p.RealTimeBuffersLost ? 0 : 2;
}

// 若系統查詢或 ETW 收尾卡住，只停止這一次建立的追蹤工作階段。
ULONG cleanup_trace(CONTROLTRACE_ID session) {
    if (!session) return ERROR_SUCCESS;
    try {
        const auto result = std::make_shared<std::atomic<ULONG>>(ERROR_TIMEOUT);
        std::thread cleanup([session, result] {
            struct {
                EVENT_TRACE_PROPERTIES properties{};
                wchar_t name[std::size(session_name)]{};
            } info;
            info.properties.Wnode.BufferSize = sizeof(info);
            info.properties.LoggerNameOffset = sizeof(EVENT_TRACE_PROPERTIES);
            result->store(ControlTraceW(session, nullptr, &info.properties, EVENT_TRACE_CONTROL_STOP));
        });
        if (WaitForSingleObject(cleanup.native_handle(), 2000) != WAIT_OBJECT_0) {
            cleanup.detach();
            return ERROR_TIMEOUT;
        }
        cleanup.join();
        return result->load();
    } catch (const std::system_error&) {
        return ERROR_NOT_ENOUGH_MEMORY;
    }
}

bool forward_output(HANDLE pipe, std::string& pending) {
    DWORD available = 0;
    char chunk[4096];
    if (!PeekNamedPipe(pipe, nullptr, 0, nullptr, &available, nullptr) || !available) return false;
    DWORD count = 0;
    if (!ReadFile(pipe, chunk, std::min<DWORD>(available, sizeof(chunk)), &count, nullptr) || !count) return false;
    pending.append(chunk, count);
    const auto newline = pending.rfind('\n');
    if (newline != std::string::npos) {
        std::fwrite(pending.data(), 1, newline + 1, stdout);
        pending.erase(0, newline + 1);
    }
    return true;
}

bool start_worker(DWORD seconds, HANDLE stop, HANDLE mapping, HANDLE out, HANDLE input, PROCESS_INFORMATION& child) {
    wchar_t path[32768];
    const DWORD length = GetModuleFileNameW(nullptr, path, static_cast<DWORD>(std::size(path)));
    if (!length || length >= std::size(path)) return false;
    auto command = L"\"" + std::wstring(path, length) + L"\" --capture-worker " + std::to_wstring(seconds)
        + L" " + std::to_wstring(reinterpret_cast<std::uintptr_t>(stop))
        + L" " + std::to_wstring(reinterpret_cast<std::uintptr_t>(mapping));

    SIZE_T bytes = 0;
    InitializeProcThreadAttributeList(nullptr, 1, 0, &bytes);
    std::vector<std::byte> storage(bytes);
    auto* attributes = reinterpret_cast<LPPROC_THREAD_ATTRIBUTE_LIST>(storage.data());
    if (!InitializeProcThreadAttributeList(attributes, 1, 0, &bytes)) return false;
    using Attributes = std::remove_pointer_t<LPPROC_THREAD_ATTRIBUTE_LIST>;
    const std::unique_ptr<Attributes, decltype(&DeleteProcThreadAttributeList)> saved(attributes, DeleteProcThreadAttributeList);
    HANDLE inherited[] = {stop, mapping, out, input};
    if (!UpdateProcThreadAttribute(attributes, 0, PROC_THREAD_ATTRIBUTE_HANDLE_LIST,
                                   inherited, sizeof(inherited), nullptr, nullptr)) return false;
    STARTUPINFOEXW info{};
    info.StartupInfo.cb = sizeof(info);
    info.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
    info.StartupInfo.hStdInput = input;
    info.StartupInfo.hStdOutput = out;
    info.StartupInfo.hStdError = out;
    info.lpAttributeList = attributes;
    return CreateProcessW(path, command.data(), nullptr, nullptr, TRUE, CREATE_NO_WINDOW | EXTENDED_STARTUPINFO_PRESENT,
                          nullptr, nullptr, &info.StartupInfo, &child) != 0;
}

int supervise(DWORD seconds) {
    SECURITY_ATTRIBUTES inherited{sizeof(SECURITY_ATTRIBUTES), nullptr, TRUE};
    const Handle stop(CreateEventW(&inherited, TRUE, FALSE, nullptr));
    const Handle mapping(CreateFileMappingW(INVALID_HANDLE_VALUE, &inherited, PAGE_READWRITE, 0, sizeof(CaptureState), nullptr));
    const Handle input(CreateFileW(L"NUL", GENERIC_READ, FILE_SHARE_READ | FILE_SHARE_WRITE, &inherited,
                                  OPEN_EXISTING, FILE_ATTRIBUTE_NORMAL, nullptr));
    const SharedState shared(mapping.get());
    HANDLE read_pipe = nullptr, write_pipe = nullptr;
    if (!stop.valid() || !shared.get() || !input.valid() || !CreatePipe(&read_pipe, &write_pipe, &inherited, 0)) return 1;
    const Handle reader(read_pipe);
    Handle writer(write_pipe);
    PROCESS_INFORMATION child{};
    if (!start_worker(seconds, stop.get(), mapping.get(), writer.get(), input.get(), child)) {
        std::printf("{\"kind\":\"error\",\"operation\":\"start_worker\"}\n");
        return 1;
    }
    const Handle worker(child.hProcess);
    const Handle thread(child.hThread);
    writer.reset();
    cancel_event = stop.get();
    const BOOL handler_set = SetConsoleCtrlHandler(cancel_capture, TRUE);
    const auto started = GetTickCount64();
    auto progress = started + 10'000;
    ULONGLONG stopping = 0;
    bool canceled = false;
    bool forced = false;
    std::string pending;
    while (WaitForSingleObject(worker.get(), 50) == WAIT_TIMEOUT) {
        forward_output(reader.get(), pending);
        const auto now = GetTickCount64();
        if (!stopping) {
            canceled = WaitForSingleObject(stop.get(), 0) == WAIT_OBJECT_0;
            if (canceled || now - started >= static_cast<ULONGLONG>(seconds) * 1000) {
                stopping = now;
                SetEvent(stop.get());
                std::printf("{\"kind\":\"stopping\",\"reason\":\"%s\"}\n", canceled ? "canceled" : "time_limit");
            } else if (now >= progress) {
                std::printf("{\"kind\":\"progress\",\"elapsed_ms\":%llu}\n", now - started);
                progress = now + 10'000;
            }
        } else if (now - stopping >= shutdown_grace_ms) {
            forced = true;
            TerminateProcess(worker.get(), ERROR_TIMEOUT);
            break;
        }
    }
    if (handler_set) SetConsoleCtrlHandler(cancel_capture, FALSE);
    for (unsigned chunk = 0; chunk < 16 && forward_output(reader.get(), pending); ++chunk) {}
    DWORD worker_exit = ERROR_TIMEOUT;
    if (!forced) GetExitCodeProcess(worker.get(), &worker_exit);
    const auto session = static_cast<CONTROLTRACE_ID>(InterlockedCompareExchange64(&shared.get()->session, 0, 0));
    const auto cleanup_error = worker_exit == ERROR_SUCCESS ? ERROR_SUCCESS : cleanup_trace(session);
    std::printf("{\"kind\":\"done\",\"complete\":%s,\"canceled\":%s,\"forced_stop\":%s,\"exit_code\":%lu,\"cleanup_error\":%lu}\n",
                worker_exit == ERROR_SUCCESS ? "true" : "false", canceled ? "true" : "false",
                forced ? "true" : "false", worker_exit, cleanup_error);
    return worker_exit == ERROR_SUCCESS ? 0 : 2;
}

} // namespace

int wmain(int argc, wchar_t* argv[]) {
    _setmode(_fileno(stdout), _O_BINARY);
    std::setvbuf(stdout, nullptr, _IONBF, 0);
    const bool worker = argc == 5 && wcscmp(argv[1], L"--capture-worker") == 0;
    wchar_t* end = nullptr;
    const auto seconds = worker ? wcstoul(argv[2], &end, 10) : argc == 2 ? wcstoul(argv[1], &end, 10) : 90;
    if ((!worker && argc > 2) || (end && *end) || seconds < 5 || seconds > 120) {
        std::fprintf(stderr, "OxideIme-LoL-EntryProbe-v3.exe [5-120]\n");
        return 1;
    }
    if (!worker) return supervise(seconds);

    const Handle stop(reinterpret_cast<HANDLE>(static_cast<std::uintptr_t>(wcstoull(argv[3], nullptr, 10))));
    const Handle mapping(reinterpret_cast<HANDLE>(static_cast<std::uintptr_t>(wcstoull(argv[4], nullptr, 10))));
    const SharedState shared(mapping.get());
    Image image;
    if (!stop.valid() || !shared.get() || !registered_image(image)) {
        std::printf("{\"kind\":\"error\",\"operation\":\"registered_image\"}\n");
        return 1;
    }
    return capture(image, L"League of Legends.exe", seconds * 1000, stop.get(), shared.get());
}
