#include <windows.h>

#include <iostream>
#include <string>

#include "eos_probe/process_inject.h"

namespace {

void PrintUsage() {
    std::wcerr << L"Usage: eos_probe_injector.exe --pid <pid> --dll <path>\n";
}

bool ParseArgs(int argc, wchar_t** argv, DWORD* pid, std::wstring* dll_path) {
    for (int i = 1; i < argc; ++i) {
        const std::wstring arg = argv[i];
        if (arg == L"--pid" && i + 1 < argc) {
            *pid = static_cast<DWORD>(_wtoi(argv[++i]));
        } else if (arg == L"--dll" && i + 1 < argc) {
            *dll_path = argv[++i];
        } else {
            return false;
        }
    }
    return *pid != 0 && !dll_path->empty();
}

}  // namespace

int wmain(int argc, wchar_t** argv) {
    DWORD pid = 0;
    std::wstring dll_path;
    if (!ParseArgs(argc, argv, &pid, &dll_path)) {
        PrintUsage();
        return 2;
    }

    dll_path = eos_probe::FullPath(dll_path);
    const DWORD attributes = GetFileAttributesW(dll_path.c_str());
    if (attributes == INVALID_FILE_ATTRIBUTES ||
        (attributes & FILE_ATTRIBUTE_DIRECTORY) != 0) {
        std::wcerr << L"DLL not found: " << dll_path << L"\n";
        return 2;
    }

    std::wstring error;
    if (!eos_probe::InjectDllIntoPid(pid, dll_path, &error)) {
        std::wcerr << error << L"\n";
        return 1;
    }

    std::wcout << L"Injected " << dll_path << L" into PID " << pid << L"\n";
    return 0;
}
