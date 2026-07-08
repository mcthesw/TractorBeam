use std::{ffi::c_void, mem::size_of, ptr, sync::Mutex};

use windows_sys::Win32::System::{
    LibraryLoader::GetModuleHandleW,
    Memory::{
        MEM_COMMIT, MEMORY_BASIC_INFORMATION, PAGE_GUARD, PAGE_NOACCESS, PAGE_READWRITE,
        VirtualProtect, VirtualQuery,
    },
};

const PATTERN: [Option<u8>; 14] = [
    Some(0xA1),
    None,
    None,
    None,
    None,
    Some(0x8B),
    Some(0x80),
    None,
    None,
    None,
    None,
    Some(0x83),
    Some(0xE8),
    Some(0x00),
];
const MANAGER_SLOT_OFFSET: usize = 1;
const INPUT_DELAY_OFFSET_OFFSET: usize = 7;
const ONLINE_INPUT_DELAY_OFFSET: usize = 0x2A3F4;
const DOS_E_LFANEW_OFFSET: usize = 0x3C;
const PE_SIGNATURE: u32 = 0x0000_4550;
const OPTIONAL_HEADER_OFFSET: usize = 24;
const SIZE_OF_IMAGE_OFFSET: usize = 56;

static TARGET: Mutex<Option<InputDelayTarget>> = Mutex::new(None);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct InputDelayTarget {
    manager_slot: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InputDelayMemoryError {
    TargetNotFound,
    MemoryAccessFailed,
    Internal,
}

pub fn read_current() -> Result<i32, InputDelayMemoryError> {
    let address = value_address()?;
    if !is_committed_accessible(address, size_of::<i32>()) {
        clear_cached_target();
        return Err(InputDelayMemoryError::MemoryAccessFailed);
    }
    Ok(unsafe { ptr::read_unaligned(address as *const i32) })
}

pub fn write_value(value: i32) -> Result<i32, InputDelayMemoryError> {
    let address = value_address()?;
    if !is_committed_accessible(address, size_of::<i32>()) {
        clear_cached_target();
        return Err(InputDelayMemoryError::MemoryAccessFailed);
    }
    let mut old_protect = 0_u32;
    let protect_ok = unsafe {
        VirtualProtect(
            address as *const c_void,
            size_of::<i32>(),
            PAGE_READWRITE,
            &mut old_protect,
        ) != 0
    };
    if !protect_ok {
        clear_cached_target();
        return Err(InputDelayMemoryError::MemoryAccessFailed);
    }
    unsafe {
        ptr::write_unaligned(address as *mut i32, value);
    }
    let mut unused = 0_u32;
    unsafe {
        VirtualProtect(
            address as *const c_void,
            size_of::<i32>(),
            old_protect,
            &mut unused,
        );
    }
    if !is_committed_accessible(address, size_of::<i32>()) {
        clear_cached_target();
        return Err(InputDelayMemoryError::MemoryAccessFailed);
    }
    Ok(unsafe { ptr::read_unaligned(address as *const i32) })
}

fn value_address() -> Result<usize, InputDelayMemoryError> {
    if let Some(target) = cached_target()? {
        if let Some(address) = resolve_value_address(target) {
            return Ok(address);
        }
        clear_cached_target();
    }
    let target = scan_target()?;
    let address = resolve_value_address(target).ok_or(InputDelayMemoryError::TargetNotFound)?;
    let mut guard = TARGET.lock().map_err(|_| InputDelayMemoryError::Internal)?;
    *guard = Some(target);
    Ok(address)
}

fn cached_target() -> Result<Option<InputDelayTarget>, InputDelayMemoryError> {
    TARGET
        .lock()
        .map(|guard| *guard)
        .map_err(|_| InputDelayMemoryError::Internal)
}

fn clear_cached_target() {
    if let Ok(mut guard) = TARGET.lock() {
        *guard = None;
    }
}

fn resolve_value_address(target: InputDelayTarget) -> Option<usize> {
    if !is_committed_accessible(target.manager_slot, size_of::<usize>()) {
        return None;
    }
    let manager = unsafe { ptr::read_unaligned(target.manager_slot as *const u32) as usize };
    if manager == 0 {
        return None;
    }
    let address = manager.checked_add(ONLINE_INPUT_DELAY_OFFSET)?;
    is_committed_accessible(address, size_of::<i32>()).then_some(address)
}

fn scan_target() -> Result<InputDelayTarget, InputDelayMemoryError> {
    let (base, size) = main_module_range()?;
    let bytes = unsafe { std::slice::from_raw_parts(base as *const u8, size) };
    let mut found = None;
    for index in 0..=bytes.len().saturating_sub(PATTERN.len()) {
        if !pattern_matches(&bytes[index..index + PATTERN.len()]) {
            continue;
        }
        if found.is_some() {
            super::bridge::log_warn("input_delay_target_ambiguous");
            return Err(InputDelayMemoryError::TargetNotFound);
        }
        let manager_slot = read_u32(&bytes[index + MANAGER_SLOT_OFFSET..])? as usize;
        let instruction_offset = read_u32(&bytes[index + INPUT_DELAY_OFFSET_OFFSET..])? as usize;
        super::bridge::log_info(format!(
            "input_delay_target_resolved instruction_offset=0x{instruction_offset:X} value_offset=0x{ONLINE_INPUT_DELAY_OFFSET:X}"
        ));
        found = Some(InputDelayTarget { manager_slot });
    }
    found.ok_or(InputDelayMemoryError::TargetNotFound)
}

fn main_module_range() -> Result<(usize, usize), InputDelayMemoryError> {
    let base = unsafe { GetModuleHandleW(ptr::null()) } as usize;
    if base == 0 {
        return Err(InputDelayMemoryError::TargetNotFound);
    }
    if unsafe { ptr::read_unaligned(base as *const u16) } != 0x5A4D {
        return Err(InputDelayMemoryError::TargetNotFound);
    }
    let e_lfanew = unsafe { ptr::read_unaligned((base + DOS_E_LFANEW_OFFSET) as *const i32) };
    if e_lfanew < 0 {
        return Err(InputDelayMemoryError::TargetNotFound);
    }
    let nt_header = base
        .checked_add(e_lfanew as usize)
        .ok_or(InputDelayMemoryError::Internal)?;
    if unsafe { ptr::read_unaligned(nt_header as *const u32) } != PE_SIGNATURE {
        return Err(InputDelayMemoryError::TargetNotFound);
    }
    let size_of_image = unsafe {
        ptr::read_unaligned(
            (nt_header + OPTIONAL_HEADER_OFFSET + SIZE_OF_IMAGE_OFFSET) as *const u32,
        )
    } as usize;
    if size_of_image < PATTERN.len() {
        return Err(InputDelayMemoryError::TargetNotFound);
    }
    Ok((base, size_of_image))
}

fn pattern_matches(bytes: &[u8]) -> bool {
    PATTERN
        .iter()
        .zip(bytes)
        .all(|(expected, actual)| expected.is_none_or(|byte| byte == *actual))
}

fn read_u32(bytes: &[u8]) -> Result<u32, InputDelayMemoryError> {
    let prefix = bytes
        .get(..size_of::<u32>())
        .ok_or(InputDelayMemoryError::Internal)?;
    let mut out = [0_u8; size_of::<u32>()];
    out.copy_from_slice(prefix);
    Ok(u32::from_le_bytes(out))
}

fn is_committed_accessible(address: usize, size: usize) -> bool {
    let Some(region) = query_memory(address) else {
        return false;
    };
    let region_start = region.BaseAddress as usize;
    let region_end = region_start.saturating_add(region.RegionSize);
    let Some(end) = address.checked_add(size) else {
        return false;
    };
    region.State == MEM_COMMIT
        && protection_allows_access(region.Protect)
        && address >= region_start
        && end <= region_end
}

fn query_memory(address: usize) -> Option<MEMORY_BASIC_INFORMATION> {
    let mut info = unsafe { std::mem::zeroed::<MEMORY_BASIC_INFORMATION>() };
    let size = unsafe {
        VirtualQuery(
            address as *const c_void,
            &mut info,
            size_of::<MEMORY_BASIC_INFORMATION>(),
        )
    };
    (size == size_of::<MEMORY_BASIC_INFORMATION>()).then_some(info)
}

fn protection_allows_access(protect: u32) -> bool {
    protect & PAGE_NOACCESS == 0 && protect & PAGE_GUARD == 0
}
