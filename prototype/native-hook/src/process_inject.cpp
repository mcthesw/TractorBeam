#include "eos_probe/process_inject.h"

#include <sstream>

namespace eos_probe {
namespace {

std::wstring LastErrorMessage(const wchar_t* operation) {
    std::wostringstream out;
    out << operation << L" failed: " << GetLastError();
    return out.str();
}

}  // namespace

std::wstring FullPath(const std::wstring& path) {
    wchar_t buffer[MAX_PATH] = {};
    const DWORD size = GetFullPathNameW(path.c_str(), MAX_PATH, buffer, nullptr);
    if (size == 0 || size >= MAX_PATH) {
        return path;
    }
    return buffer;
}

bool InjectDllIntoProcess(HANDLE process,
                          const std::wstring& dll_path,
                          std::wstring* error) {
    if (process == nullptr) {
        if (error != nullptr) {
            *error = L"Invalid process handle.";
        }
        return false;
    }

    const SIZE_T bytes = (dll_path.size() + 1) * sizeof(wchar_t);
    LPVOID remote_path =
        VirtualAllocEx(process, nullptr, bytes, MEM_COMMIT, PAGE_READWRITE);
    if (remote_path == nullptr) {
        if (error != nullptr) {
            *error = LastErrorMessage(L"VirtualAllocEx");
        }
        return false;
    }

    if (!WriteProcessMemory(process, remote_path, dll_path.c_str(), bytes, nullptr)) {
        if (error != nullptr) {
            *error = LastErrorMessage(L"WriteProcessMemory");
        }
        VirtualFreeEx(process, remote_path, 0, MEM_RELEASE);
        return false;
    }

    HMODULE kernel32 = GetModuleHandleW(L"kernel32.dll");
    auto* load_library = reinterpret_cast<LPTHREAD_START_ROUTINE>(
        GetProcAddress(kernel32, "LoadLibraryW"));
    if (load_library == nullptr) {
        if (error != nullptr) {
            *error = L"GetProcAddress(LoadLibraryW) failed.";
        }
        VirtualFreeEx(process, remote_path, 0, MEM_RELEASE);
        return false;
    }

    HANDLE thread =
        CreateRemoteThread(process, nullptr, 0, load_library, remote_path, 0, nullptr);
    if (thread == nullptr) {
        if (error != nullptr) {
            *error = LastErrorMessage(L"CreateRemoteThread");
        }
        VirtualFreeEx(process, remote_path, 0, MEM_RELEASE);
        return false;
    }

    WaitForSingleObject(thread, 10000);
    DWORD exit_code = 0;
    GetExitCodeThread(thread, &exit_code);

    CloseHandle(thread);
    VirtualFreeEx(process, remote_path, 0, MEM_RELEASE);

    if (exit_code == 0) {
        if (error != nullptr) {
            *error = L"LoadLibraryW returned null in remote process.";
        }
        return false;
    }

    return true;
}

bool InjectDllIntoPid(DWORD pid,
                      const std::wstring& dll_path,
                      std::wstring* error) {
    HANDLE process = OpenProcess(PROCESS_CREATE_THREAD |
                                     PROCESS_QUERY_INFORMATION |
                                     PROCESS_VM_OPERATION |
                                     PROCESS_VM_WRITE |
                                     PROCESS_VM_READ,
                                 FALSE,
                                 pid);
    if (process == nullptr) {
        if (error != nullptr) {
            *error = LastErrorMessage(L"OpenProcess");
        }
        return false;
    }

    const bool ok = InjectDllIntoProcess(process, dll_path, error);
    CloseHandle(process);
    return ok;
}

}  // namespace eos_probe
