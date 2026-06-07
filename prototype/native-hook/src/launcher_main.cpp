#include <windows.h>

#include <iostream>
#include <string>

#include "eos_probe/process_inject.h"

namespace {

void PrintUsage() {
    std::wcerr << L"Usage: eos_probe_launcher.exe --exe <isaac-ng.exe> "
                  L"--dll <isaac_eos_probe.dll> [--args <launch args>]\n";
}

bool ParseArgs(int argc,
               wchar_t** argv,
               std::wstring* exe_path,
               std::wstring* dll_path,
               std::wstring* launch_args) {
    for (int i = 1; i < argc; ++i) {
        const std::wstring arg = argv[i];
        if (arg == L"--exe" && i + 1 < argc) {
            *exe_path = argv[++i];
        } else if (arg == L"--dll" && i + 1 < argc) {
            *dll_path = argv[++i];
        } else if (arg == L"--args" && i + 1 < argc) {
            *launch_args = argv[++i];
        } else {
            return false;
        }
    }
    return !exe_path->empty() && !dll_path->empty();
}

std::wstring Quote(const std::wstring& value) {
    return L"\"" + value + L"\"";
}

}  // namespace

int wmain(int argc, wchar_t** argv) {
    std::wstring exe_path;
    std::wstring dll_path;
    std::wstring launch_args;
    if (!ParseArgs(argc, argv, &exe_path, &dll_path, &launch_args)) {
        PrintUsage();
        return 2;
    }

    exe_path = eos_probe::FullPath(exe_path);
    dll_path = eos_probe::FullPath(dll_path);

    if (GetFileAttributesW(exe_path.c_str()) == INVALID_FILE_ATTRIBUTES) {
        std::wcerr << L"Executable not found: " << exe_path << L"\n";
        return 2;
    }
    if (GetFileAttributesW(dll_path.c_str()) == INVALID_FILE_ATTRIBUTES) {
        std::wcerr << L"DLL not found: " << dll_path << L"\n";
        return 2;
    }

    std::wstring command_line = Quote(exe_path);
    if (!launch_args.empty()) {
        command_line += L" ";
        command_line += launch_args;
    }

    STARTUPINFOW startup_info = {};
    startup_info.cb = sizeof(startup_info);
    PROCESS_INFORMATION process_info = {};

    if (!CreateProcessW(exe_path.c_str(),
                        command_line.data(),
                        nullptr,
                        nullptr,
                        FALSE,
                        CREATE_SUSPENDED,
                        nullptr,
                        nullptr,
                        &startup_info,
                        &process_info)) {
        std::wcerr << L"CreateProcessW failed: " << GetLastError() << L"\n";
        return 1;
    }

    std::wstring error;
    const bool injected =
        eos_probe::InjectDllIntoProcess(process_info.hProcess, dll_path, &error);
    if (!injected) {
        std::wcerr << error << L"\n";
        TerminateProcess(process_info.hProcess, 1);
        CloseHandle(process_info.hThread);
        CloseHandle(process_info.hProcess);
        return 1;
    }

    ResumeThread(process_info.hThread);
    std::wcout << L"Launched PID " << process_info.dwProcessId
               << L" with probe injected\n";

    CloseHandle(process_info.hThread);
    CloseHandle(process_info.hProcess);
    return 0;
}
