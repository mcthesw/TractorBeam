#include <windows.h>

#include "eos_probe/eos_hooks.h"
#include "eos_probe/telemetry.h"

#if !defined(_M_IX86) && !defined(__i386__)
#error "isaac-ng.exe is 32-bit; build isaac_eos_probe as x86/Win32."
#endif

namespace {

DWORD WINAPI InstallThread(LPVOID) {
    eos_probe::OpenTelemetry();

    for (int attempt = 0; attempt < 300; ++attempt) {
        if (GetModuleHandleW(L"EOSSDK-Win32-Shipping.dll") != nullptr) {
            break;
        }
        Sleep(100);
    }

    if (eos_probe::InstallEosHooks()) {
        eos_probe::LogInstallEvent("ok");
    } else {
        eos_probe::LogInstallEvent("failed");
    }

    return 0;
}

}  // namespace

BOOL APIENTRY DllMain(HMODULE module, DWORD reason, LPVOID) {
    if (reason == DLL_PROCESS_ATTACH) {
        DisableThreadLibraryCalls(module);
        HANDLE thread = CreateThread(nullptr, 0, InstallThread, nullptr, 0, nullptr);
        if (thread != nullptr) {
            CloseHandle(thread);
        }
    } else if (reason == DLL_PROCESS_DETACH) {
        eos_probe::ShutdownEosHooks();
        eos_probe::CloseTelemetry();
    }
    return TRUE;
}
