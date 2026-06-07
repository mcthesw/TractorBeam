#pragma once

#include <windows.h>

#include <string>

namespace eos_probe {

bool InjectDllIntoProcess(HANDLE process,
                          const std::wstring& dll_path,
                          std::wstring* error);

bool InjectDllIntoPid(DWORD pid,
                      const std::wstring& dll_path,
                      std::wstring* error);

std::wstring FullPath(const std::wstring& path);

}  // namespace eos_probe
