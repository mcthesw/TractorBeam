#include "eos_probe/iat_patch.h"

#include <cstring>

#include "eos_probe/telemetry.h"

namespace eos_probe {
namespace {

bool IsValidPeImage(const std::uint8_t* base) {
    const auto* dos = reinterpret_cast<const IMAGE_DOS_HEADER*>(base);
    if (dos->e_magic != IMAGE_DOS_SIGNATURE) {
        return false;
    }

    const auto* nt = reinterpret_cast<const IMAGE_NT_HEADERS*>(base + dos->e_lfanew);
    return nt->Signature == IMAGE_NT_SIGNATURE;
}

bool SameModuleName(const char* left, const char* right) {
    return _stricmp(left, right) == 0;
}

}  // namespace

bool PatchImportAddressTable(HMODULE module,
                             const char* import_module,
                             ImportPatch* patches,
                             std::size_t patch_count) {
    if (module == nullptr || import_module == nullptr || patches == nullptr) {
        return false;
    }

    auto* base = reinterpret_cast<std::uint8_t*>(module);
    if (!IsValidPeImage(base)) {
        return false;
    }

    const auto* dos = reinterpret_cast<const IMAGE_DOS_HEADER*>(base);
    const auto* nt = reinterpret_cast<const IMAGE_NT_HEADERS*>(base + dos->e_lfanew);
    const auto& directory =
        nt->OptionalHeader.DataDirectory[IMAGE_DIRECTORY_ENTRY_IMPORT];
    if (directory.VirtualAddress == 0 || directory.Size == 0) {
        return false;
    }

    auto* descriptor =
        reinterpret_cast<IMAGE_IMPORT_DESCRIPTOR*>(base + directory.VirtualAddress);

    bool patched_any = false;
    for (; descriptor->Name != 0; ++descriptor) {
        const char* module_name =
            reinterpret_cast<const char*>(base + descriptor->Name);
        if (!SameModuleName(module_name, import_module)) {
            continue;
        }

        const DWORD original_thunk_rva =
            descriptor->OriginalFirstThunk != 0 ? descriptor->OriginalFirstThunk
                                                : descriptor->FirstThunk;
        auto* original_thunk =
            reinterpret_cast<IMAGE_THUNK_DATA*>(base + original_thunk_rva);
        auto* thunk =
            reinterpret_cast<IMAGE_THUNK_DATA*>(base + descriptor->FirstThunk);
        if (original_thunk == nullptr || thunk == nullptr) {
            continue;
        }

        for (; original_thunk->u1.AddressOfData != 0; ++original_thunk, ++thunk) {
            if (IMAGE_SNAP_BY_ORDINAL(original_thunk->u1.Ordinal)) {
                continue;
            }

            const auto* import_by_name = reinterpret_cast<const IMAGE_IMPORT_BY_NAME*>(
                base + original_thunk->u1.AddressOfData);
            const char* symbol_name =
                reinterpret_cast<const char*>(import_by_name->Name);

            for (std::size_t i = 0; i < patch_count; ++i) {
                ImportPatch& patch = patches[i];
                if (patch.symbol == nullptr ||
                    std::strcmp(symbol_name, patch.symbol) != 0) {
                    continue;
                }

                DWORD old_protect = 0;
                auto* slot = reinterpret_cast<void**>(&thunk->u1.Function);
                if (!VirtualProtect(slot,
                                    sizeof(void*),
                                    PAGE_EXECUTE_READWRITE,
                                    &old_protect)) {
                    LogPatchEvent(patch.symbol, "protect_failed");
                    continue;
                }

                if (patch.original != nullptr && *patch.original == nullptr) {
                    *patch.original = *slot;
                }
                *slot = patch.replacement;

                DWORD unused = 0;
                VirtualProtect(slot, sizeof(void*), old_protect, &unused);
                FlushInstructionCache(GetCurrentProcess(), slot, sizeof(void*));
                LogPatchEvent(patch.symbol, "patched");
                patched_any = true;
            }
        }
    }

    return patched_any;
}

}  // namespace eos_probe
