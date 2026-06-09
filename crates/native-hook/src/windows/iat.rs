use std::{
    ffi::{CStr, c_void},
    ptr,
    sync::atomic::{AtomicPtr, Ordering},
};

use windows_sys::Win32::{
    Foundation::HMODULE,
    System::{
        Diagnostics::Debug::{FlushInstructionCache, IMAGE_DIRECTORY_ENTRY_IMPORT},
        Memory::{PAGE_EXECUTE_READWRITE, VirtualProtect},
        SystemServices::{
            IMAGE_DOS_HEADER, IMAGE_DOS_SIGNATURE, IMAGE_IMPORT_BY_NAME, IMAGE_IMPORT_DESCRIPTOR,
            IMAGE_NT_SIGNATURE, IMAGE_ORDINAL_FLAG32,
        },
        Threading::GetCurrentProcess,
        WindowsProgramming::IMAGE_THUNK_DATA32,
    },
};

pub struct ImportPatch {
    pub symbol: &'static str,
    pub replacement: *mut c_void,
    pub original: &'static AtomicPtr<c_void>,
}

pub unsafe fn patch_imports(module: HMODULE, import_module: &str, patches: &[ImportPatch]) -> bool {
    if module.is_null() {
        return false;
    }

    let base = module.cast::<u8>();
    let dos = base.cast::<IMAGE_DOS_HEADER>();
    if unsafe { (*dos).e_magic } != IMAGE_DOS_SIGNATURE {
        return false;
    }

    let nt = unsafe { base.add((*dos).e_lfanew as usize) }
        .cast::<windows_sys::Win32::System::Diagnostics::Debug::IMAGE_NT_HEADERS32>();
    if unsafe { (*nt).Signature } != IMAGE_NT_SIGNATURE {
        return false;
    }

    let directory =
        unsafe { (*nt).OptionalHeader.DataDirectory[IMAGE_DIRECTORY_ENTRY_IMPORT as usize] };
    if directory.VirtualAddress == 0 || directory.Size == 0 {
        return false;
    }

    let mut descriptor =
        unsafe { base.add(directory.VirtualAddress as usize) }.cast::<IMAGE_IMPORT_DESCRIPTOR>();
    let mut patched_any = false;

    while unsafe { (*descriptor).Name } != 0 {
        let module_name = unsafe { CStr::from_ptr(base.add((*descriptor).Name as usize).cast()) };
        if module_name
            .to_string_lossy()
            .eq_ignore_ascii_case(import_module)
        {
            patched_any |= unsafe { patch_descriptor(base, descriptor, patches) };
        }
        descriptor = unsafe { descriptor.add(1) };
    }

    patched_any
}

unsafe fn patch_descriptor(
    base: *mut u8,
    descriptor: *mut IMAGE_IMPORT_DESCRIPTOR,
    patches: &[ImportPatch],
) -> bool {
    let original_first_thunk = unsafe { (*descriptor).Anonymous.OriginalFirstThunk };
    let original_rva = if original_first_thunk == 0 {
        unsafe { (*descriptor).FirstThunk }
    } else {
        original_first_thunk
    };

    let mut original_thunk =
        unsafe { base.add(original_rva as usize) }.cast::<IMAGE_THUNK_DATA32>();
    let mut thunk =
        unsafe { base.add((*descriptor).FirstThunk as usize) }.cast::<IMAGE_THUNK_DATA32>();
    let mut patched_any = false;

    while unsafe { (*original_thunk).u1.AddressOfData } != 0 {
        let ordinal = unsafe { (*original_thunk).u1.Ordinal };
        if ordinal & IMAGE_ORDINAL_FLAG32 == 0 {
            let import_by_name = unsafe { base.add((*original_thunk).u1.AddressOfData as usize) }
                .cast::<IMAGE_IMPORT_BY_NAME>();
            let symbol = unsafe { CStr::from_ptr((*import_by_name).Name.as_ptr()) };
            for patch in patches {
                if symbol.to_bytes() == patch.symbol.as_bytes() {
                    unsafe {
                        patch_slot(thunk, patch);
                    }
                    patched_any = true;
                }
            }
        }
        original_thunk = unsafe { original_thunk.add(1) };
        thunk = unsafe { thunk.add(1) };
    }

    patched_any
}

unsafe fn patch_slot(thunk: *mut IMAGE_THUNK_DATA32, patch: &ImportPatch) {
    let slot = unsafe { ptr::addr_of_mut!((*thunk).u1.Function) };
    let mut old_protect = 0;
    if unsafe {
        VirtualProtect(
            slot.cast(),
            size_of::<u32>(),
            PAGE_EXECUTE_READWRITE,
            &mut old_protect,
        )
    } == 0
    {
        return;
    }

    let original = unsafe { *slot as usize as *mut c_void };
    let _ = patch.original.compare_exchange(
        ptr::null_mut(),
        original,
        Ordering::SeqCst,
        Ordering::SeqCst,
    );
    unsafe {
        *slot = patch.replacement as u32;
    }

    let mut unused = 0;
    unsafe {
        VirtualProtect(slot.cast(), size_of::<u32>(), old_protect, &mut unused);
        FlushInstructionCache(GetCurrentProcess(), slot.cast(), size_of::<u32>());
    }
}
