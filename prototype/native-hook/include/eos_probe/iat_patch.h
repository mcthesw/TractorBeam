#pragma once

#include <windows.h>

#include <cstddef>

namespace eos_probe {

struct ImportPatch {
    const char* symbol;
    void* replacement;
    void** original;
};

bool PatchImportAddressTable(HMODULE module,
                             const char* import_module,
                             ImportPatch* patches,
                             std::size_t patch_count);

}  // namespace eos_probe
