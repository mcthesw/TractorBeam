use std::{
    ffi::{CStr, c_char, c_void},
    ptr,
    sync::atomic::{AtomicBool, AtomicPtr, AtomicU32, Ordering},
};

use tractor_beam_hook_ipc::HookStartupFailure;
use windows_sys::Win32::{
    Foundation::HMODULE,
    System::{
        Diagnostics::Debug::FlushInstructionCache,
        LibraryLoader::{GetModuleHandleW, GetProcAddress},
        Memory::{PAGE_EXECUTE_READWRITE, VirtualProtect},
        Threading::GetCurrentProcess,
    },
};

use super::{bridge, iat};

type SteamFindOrCreateUserInterfaceFn =
    unsafe extern "C" fn(steam_user: i32, version: *const c_char) -> *mut c_void;
type SteamGetHSteamUserFn = unsafe extern "C" fn() -> i32;
type SteamUserFn = unsafe extern "C" fn() -> *mut c_void;
type SteamUserGetSteamIdFn = unsafe extern "C" fn(*mut c_void) -> u64;
type SteamRunCallbacksFn = unsafe extern "C" fn();
type SteamSendP2PPacketFn = unsafe extern "thiscall" fn(
    this: *mut c_void,
    remote: u64,
    data: *const c_void,
    bytes: u32,
    send_type: i32,
    channel: i32,
) -> bool;
type SteamIsP2PPacketAvailableFn =
    unsafe extern "thiscall" fn(this: *mut c_void, bytes: *mut u32, channel: i32) -> bool;
type SteamReadP2PPacketFn = unsafe extern "thiscall" fn(
    this: *mut c_void,
    destination: *mut c_void,
    max_bytes: u32,
    bytes_read: *mut u32,
    remote: *mut u64,
    channel: i32,
) -> bool;
type SteamGetP2PSessionStateFn =
    unsafe extern "thiscall" fn(this: *mut c_void, remote: u64, state: *mut c_void) -> bool;

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
struct SteamP2PSessionState006 {
    connection_active: u8,
    connecting: u8,
    session_error: u8,
    using_relay: u8,
    bytes_queued_for_send: i32,
    packets_queued_for_send: i32,
    remote_ip: u32,
    remote_port: u16,
}

static ORIGINAL_FIND_INTERFACE: AtomicPtr<c_void> = AtomicPtr::new(ptr::null_mut());
static ORIGINAL_RUN_CALLBACKS: AtomicPtr<c_void> = AtomicPtr::new(ptr::null_mut());
static ORIGINAL_SEND: AtomicPtr<c_void> = AtomicPtr::new(ptr::null_mut());
static ORIGINAL_AVAILABLE: AtomicPtr<c_void> = AtomicPtr::new(ptr::null_mut());
static ORIGINAL_READ: AtomicPtr<c_void> = AtomicPtr::new(ptr::null_mut());
static ORIGINAL_SESSION_STATE: AtomicPtr<c_void> = AtomicPtr::new(ptr::null_mut());
static STEAM_NETWORKING_VTABLE: AtomicPtr<c_void> = AtomicPtr::new(ptr::null_mut());
static STEAM_NETWORKING_HOOKED: AtomicBool = AtomicBool::new(false);
static STEAM_NETWORKING_HOOKING: AtomicBool = AtomicBool::new(false);
static SESSION_STATE_CALLS: AtomicU32 = AtomicU32::new(0);
static RUN_CALLBACK_CALLS: AtomicU32 = AtomicU32::new(0);
const STEAM_NETWORKING_RETRY_CALLBACK_INTERVAL: u32 = 60;

pub unsafe fn install_hooks() {
    let module = unsafe { GetModuleHandleW(ptr::null()) };
    let steam_module = unsafe { GetModuleHandleW(wide_null("steam_api.dll").as_ptr()) };
    let patches = [
        iat::ImportPatch {
            symbol: "SteamInternal_FindOrCreateUserInterface",
            replacement: hook_find_or_create_user_interface as *mut c_void,
            original: &ORIGINAL_FIND_INTERFACE,
        },
        iat::ImportPatch {
            symbol: "SteamAPI_RunCallbacks",
            replacement: hook_run_callbacks as *mut c_void,
            original: &ORIGINAL_RUN_CALLBACKS,
        },
    ];
    let patched = unsafe { iat::patch_imports(module, "steam_api.dll", &patches) };
    bridge::log_info(format!(
        "steam_iat_patch patched={patched} find_original={} callbacks_original={}",
        !ORIGINAL_FIND_INTERFACE.load(Ordering::SeqCst).is_null(),
        !ORIGINAL_RUN_CALLBACKS.load(Ordering::SeqCst).is_null()
    ));
    if !patched {
        bridge::report_hook_failure(HookStartupFailure::SteamApiImports);
        return;
    }
    unsafe {
        install_existing_steam_networking_interface(steam_module);
    }
}

unsafe extern "C" fn hook_find_or_create_user_interface(
    steam_user: i32,
    version: *const c_char,
) -> *mut c_void {
    let result = if let Some(original) =
        original_fn::<SteamFindOrCreateUserInterfaceFn>(&ORIGINAL_FIND_INTERFACE)
    {
        unsafe { original(steam_user, version) }
    } else {
        ptr::null_mut()
    };

    if !version.is_null() && unsafe { CStr::from_ptr(version) }.to_bytes() == b"SteamNetworking006"
    {
        bridge::log_debug(format!(
            "steam_find_interface steam_user={steam_user} version=SteamNetworking006 result={:p}",
            result
        ));
        unsafe {
            install_steam_networking_hooks(result);
        }
    }

    result
}

unsafe extern "C" fn hook_run_callbacks() {
    let callback_call = RUN_CALLBACK_CALLS.fetch_add(1, Ordering::Relaxed) + 1;
    if should_sample_callback(callback_call) {
        bridge::log_trace(format!(
            "steam_run_callbacks call={callback_call} original={}",
            !ORIGINAL_RUN_CALLBACKS.load(Ordering::SeqCst).is_null()
        ));
    }
    if let Some(original) = original_fn::<SteamRunCallbacksFn>(&ORIGINAL_RUN_CALLBACKS) {
        unsafe { original() };
    }
    if should_retry_steam_networking(callback_call) {
        if STEAM_NETWORKING_HOOKED.load(Ordering::SeqCst) {
            unsafe {
                verify_registered_steam_networking_vtable();
            }
        } else {
            let steam_module = unsafe { GetModuleHandleW(wide_null("steam_api.dll").as_ptr()) };
            unsafe {
                install_existing_steam_networking_interface(steam_module);
            }
        }
    }
}

unsafe fn install_steam_networking_hooks(interface: *mut c_void) {
    if interface.is_null() {
        return;
    }
    let vtable = unsafe { *(interface.cast::<*mut *mut c_void>()) };
    STEAM_NETWORKING_VTABLE.store(vtable.cast(), Ordering::SeqCst);
    unsafe {
        install_steam_networking_vtable(vtable);
    }
}

unsafe fn verify_registered_steam_networking_vtable() {
    let vtable = STEAM_NETWORKING_VTABLE.load(Ordering::SeqCst);
    if !vtable.is_null() {
        unsafe {
            install_steam_networking_vtable(vtable.cast());
        }
    }
}

unsafe fn install_steam_networking_vtable(vtable: *mut *mut c_void) {
    if STEAM_NETWORKING_HOOKING.swap(true, Ordering::SeqCst) {
        return;
    }

    if vtable.is_null() {
        STEAM_NETWORKING_HOOKING.store(false, Ordering::SeqCst);
        return;
    }
    if unsafe { steam_networking_vtable_is_hooked(vtable) } {
        STEAM_NETWORKING_HOOKING.store(false, Ordering::SeqCst);
        return;
    }

    let first_install = !STEAM_NETWORKING_HOOKED.load(Ordering::SeqCst);
    bridge::log_info(format!(
        "steam_networking006_hooking rehook={}",
        !first_install
    ));
    let installed = unsafe {
        patch_vtable_slot(
            vtable,
            0,
            hook_send_p2p_packet as *mut c_void,
            &ORIGINAL_SEND,
        ) && patch_vtable_slot(
            vtable,
            1,
            hook_is_p2p_packet_available as *mut c_void,
            &ORIGINAL_AVAILABLE,
        ) && patch_vtable_slot(
            vtable,
            2,
            hook_read_p2p_packet as *mut c_void,
            &ORIGINAL_READ,
        ) && patch_vtable_slot(
            vtable,
            6,
            hook_get_p2p_session_state as *mut c_void,
            &ORIGINAL_SESSION_STATE,
        )
    };
    STEAM_NETWORKING_HOOKING.store(false, Ordering::SeqCst);
    if !installed {
        bridge::report_hook_failure(HookStartupFailure::SteamNetworkingHooks);
        return;
    }

    STEAM_NETWORKING_HOOKED.store(true, Ordering::SeqCst);
    if first_install {
        let steam_id64 = unsafe { active_steam_id64() };
        bridge::report_hook_ready(steam_id64);
        bridge::log_info(format!(
            "steam_identity_ready available={}",
            steam_id64.is_some()
        ));
        bridge::log_info("steam_networking006_hooked");
    } else {
        bridge::log_warn("steam_networking006_rehooked");
    }
}

unsafe fn steam_networking_vtable_is_hooked(vtable: *mut *mut c_void) -> bool {
    !vtable.is_null()
        && unsafe { *vtable.add(0) == hook_send_p2p_packet as *mut c_void }
        && unsafe { *vtable.add(1) == hook_is_p2p_packet_available as *mut c_void }
        && unsafe { *vtable.add(2) == hook_read_p2p_packet as *mut c_void }
        && unsafe { *vtable.add(6) == hook_get_p2p_session_state as *mut c_void }
}

unsafe fn active_steam_id64() -> Option<u64> {
    let steam_module = unsafe { GetModuleHandleW(wide_null("steam_api.dll").as_ptr()) };
    if steam_module.is_null() {
        return None;
    }
    let steam_user =
        unsafe { export_fn::<SteamUserFn>(steam_module, b"SteamAPI_SteamUser_v023\0")? };
    let get_steam_id = unsafe {
        export_fn::<SteamUserGetSteamIdFn>(steam_module, b"SteamAPI_ISteamUser_GetSteamID\0")?
    };
    let user = unsafe { steam_user() };
    if user.is_null() {
        return None;
    }
    let steam_id64 = unsafe { get_steam_id(user) };
    (steam_id64 != 0).then_some(steam_id64)
}

unsafe fn install_existing_steam_networking_interface(steam_module: HMODULE) {
    if steam_module.is_null() {
        bridge::log_warn("steam_probe module_missing=steam_api.dll");
        bridge::report_hook_failure(HookStartupFailure::SteamApiImports);
        return;
    }

    let get_user =
        unsafe { export_fn::<SteamGetHSteamUserFn>(steam_module, b"SteamAPI_GetHSteamUser\0") };
    let find_interface = unsafe {
        export_fn::<SteamFindOrCreateUserInterfaceFn>(
            steam_module,
            b"SteamInternal_FindOrCreateUserInterface\0",
        )
    }
    .or_else(|| original_fn::<SteamFindOrCreateUserInterfaceFn>(&ORIGINAL_FIND_INTERFACE));

    let Some(find_interface) = find_interface else {
        bridge::log_warn("steam_probe find_interface_missing=true");
        return;
    };

    let steam_user = get_user.map_or(0, |get_user| unsafe { get_user() });
    let interface = unsafe { find_interface(steam_user, c"SteamNetworking006".as_ptr()) };
    bridge::log_debug(format!(
        "steam_probe steam_user={steam_user} get_user_export={} interface={:p}",
        get_user.is_some(),
        interface
    ));
    unsafe {
        install_steam_networking_hooks(interface);
    }
}

unsafe fn export_fn<T>(module: HMODULE, name: &[u8]) -> Option<T> {
    let function = unsafe { GetProcAddress(module, name.as_ptr()) }?;
    let pointer = function as usize as *mut c_void;
    Some(unsafe { std::mem::transmute_copy(&pointer) })
}

unsafe fn patch_vtable_slot(
    vtable: *mut *mut c_void,
    index: usize,
    replacement: *mut c_void,
    original: &'static AtomicPtr<c_void>,
) -> bool {
    if vtable.is_null() {
        return false;
    }
    let slot = unsafe { vtable.add(index) };
    if unsafe { *slot == replacement } {
        return true;
    }
    let mut old_protect = 0;
    if unsafe {
        VirtualProtect(
            slot.cast(),
            size_of::<*mut c_void>(),
            PAGE_EXECUTE_READWRITE,
            &mut old_protect,
        )
    } == 0
    {
        bridge::log_error(format!("steam_vtable_patch_failed index={index}"));
        return false;
    }

    let current = unsafe { *slot };
    if current.is_null() {
        bridge::log_error(format!("steam_vtable_original_missing index={index}"));
        restore_vtable_protection(slot, old_protect);
        return false;
    }
    if current == replacement {
        restore_vtable_protection(slot, old_protect);
        return true;
    }
    original.store(current, Ordering::SeqCst);
    unsafe {
        *slot = replacement;
    }

    restore_vtable_protection(slot, old_protect);
    true
}

fn restore_vtable_protection(slot: *mut *mut c_void, old_protect: u32) {
    let mut unused = 0;
    unsafe {
        VirtualProtect(
            slot.cast(),
            size_of::<*mut c_void>(),
            old_protect,
            &mut unused,
        );
        FlushInstructionCache(GetCurrentProcess(), slot.cast(), size_of::<*mut c_void>());
    }
}

unsafe extern "thiscall" fn hook_send_p2p_packet(
    this: *mut c_void,
    remote: u64,
    data: *const c_void,
    bytes: u32,
    send_type: i32,
    channel: i32,
) -> bool {
    let bridged = bridge::send_packet(remote, data.cast(), bytes, send_type, channel);
    if bridge::mode() == bridge::BridgeMode::Replace {
        bridged
    } else if let Some(original) = original_fn::<SteamSendP2PPacketFn>(&ORIGINAL_SEND) {
        unsafe { original(this, remote, data, bytes, send_type, channel) }
    } else {
        false
    }
}

unsafe extern "thiscall" fn hook_is_p2p_packet_available(
    this: *mut c_void,
    bytes: *mut u32,
    channel: i32,
) -> bool {
    if bridge::has_packet(channel, bytes) {
        return true;
    }
    if (bridge::mode() != bridge::BridgeMode::Replace || bridge::should_fallback_to_steam())
        && let Some(original) = original_fn::<SteamIsP2PPacketAvailableFn>(&ORIGINAL_AVAILABLE)
    {
        return unsafe { original(this, bytes, channel) };
    }
    false
}

unsafe extern "thiscall" fn hook_read_p2p_packet(
    this: *mut c_void,
    destination: *mut c_void,
    max_bytes: u32,
    bytes_read: *mut u32,
    remote: *mut u64,
    channel: i32,
) -> bool {
    if bridge::read_packet(channel, destination.cast(), max_bytes, bytes_read, remote) {
        return true;
    }
    if (bridge::mode() != bridge::BridgeMode::Replace || bridge::should_fallback_to_steam())
        && let Some(original) = original_fn::<SteamReadP2PPacketFn>(&ORIGINAL_READ)
    {
        return unsafe { original(this, destination, max_bytes, bytes_read, remote, channel) };
    }
    false
}

fn should_sample(call: u32) -> bool {
    call <= 32 || call.is_multiple_of(1000)
}

fn should_sample_callback(call: u32) -> bool {
    call <= 8 || call.is_multiple_of(5_000)
}

fn should_retry_steam_networking(call: u32) -> bool {
    call == 1 || call.is_multiple_of(STEAM_NETWORKING_RETRY_CALLBACK_INTERVAL)
}

unsafe extern "thiscall" fn hook_get_p2p_session_state(
    this: *mut c_void,
    remote: u64,
    state: *mut c_void,
) -> bool {
    let session_call = SESSION_STATE_CALLS.fetch_add(1, Ordering::Relaxed) + 1;
    let result =
        if let Some(original) = original_fn::<SteamGetP2PSessionStateFn>(&ORIGINAL_SESSION_STATE) {
            unsafe { original(this, remote, state) }
        } else {
            false
        };

    if result || should_sample(session_call) {
        let level = if result {
            bridge::HookLogLevel::Debug
        } else {
            bridge::HookLogLevel::Trace
        };
        if let Some(session_state) = result.then(|| read_session_state(state)).flatten() {
            bridge::log(
                level,
                format!(
                    "steam_session_state call={session_call} remote={remote} steam_result={result} active={} connecting={} error={} relay={} queued_bytes={} queued_packets={}",
                    session_state.connection_active,
                    session_state.connecting,
                    session_state.session_error,
                    session_state.using_relay,
                    session_state.bytes_queued_for_send,
                    session_state.packets_queued_for_send
                ),
            );
        } else {
            bridge::log(
                level,
                format!(
                    "steam_session_state call={session_call} remote={remote} steam_result={result}"
                ),
            );
        }
    }

    result
}

fn read_session_state(state: *const c_void) -> Option<SteamP2PSessionState006> {
    if state.is_null() {
        None
    } else {
        Some(unsafe { state.cast::<SteamP2PSessionState006>().read() })
    }
}

fn original_fn<T>(slot: &AtomicPtr<c_void>) -> Option<T> {
    let pointer = slot.load(Ordering::SeqCst);
    if pointer.is_null() {
        None
    } else {
        Some(unsafe { std::mem::transmute_copy(&pointer) })
    }
}

fn wide_null(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retries_steam_networking_on_first_and_bounded_callback_intervals() {
        assert!(should_retry_steam_networking(1));
        assert!(!should_retry_steam_networking(2));
        assert!(!should_retry_steam_networking(59));
        assert!(should_retry_steam_networking(60));
        assert!(should_retry_steam_networking(120));
    }

    unsafe extern "C" fn test_original_one() {}
    unsafe extern "C" fn test_original_two() {}
    unsafe extern "C" fn test_replacement() {}

    static TEST_ORIGINAL: AtomicPtr<c_void> = AtomicPtr::new(ptr::null_mut());

    #[test]
    fn vtable_patch_refreshes_original_after_external_restore() {
        TEST_ORIGINAL.store(ptr::null_mut(), Ordering::SeqCst);
        let original_one = test_original_one as *mut c_void;
        let original_two = test_original_two as *mut c_void;
        let replacement = test_replacement as *mut c_void;
        let mut vtable = [original_one];

        assert!(unsafe { patch_vtable_slot(vtable.as_mut_ptr(), 0, replacement, &TEST_ORIGINAL) });
        assert_eq!(vtable[0], replacement);
        assert_eq!(TEST_ORIGINAL.load(Ordering::SeqCst), original_one);

        assert!(unsafe { patch_vtable_slot(vtable.as_mut_ptr(), 0, replacement, &TEST_ORIGINAL) });
        assert_eq!(TEST_ORIGINAL.load(Ordering::SeqCst), original_one);

        vtable[0] = original_two;
        assert!(unsafe { patch_vtable_slot(vtable.as_mut_ptr(), 0, replacement, &TEST_ORIGINAL) });
        assert_eq!(vtable[0], replacement);
        assert_eq!(TEST_ORIGINAL.load(Ordering::SeqCst), original_two);
    }

    #[test]
    fn registered_vtable_is_repaired_without_creating_another_interface() {
        let original = test_original_one as *mut c_void;
        let mut vtable = [original; 7];
        STEAM_NETWORKING_VTABLE.store(vtable.as_mut_ptr().cast(), Ordering::SeqCst);
        STEAM_NETWORKING_HOOKED.store(true, Ordering::SeqCst);
        STEAM_NETWORKING_HOOKING.store(false, Ordering::SeqCst);

        unsafe {
            verify_registered_steam_networking_vtable();
        }
        assert!(unsafe { steam_networking_vtable_is_hooked(vtable.as_mut_ptr()) });

        vtable[0] = original;
        unsafe {
            verify_registered_steam_networking_vtable();
        }
        assert!(unsafe { steam_networking_vtable_is_hooked(vtable.as_mut_ptr()) });

        STEAM_NETWORKING_VTABLE.store(ptr::null_mut(), Ordering::SeqCst);
        STEAM_NETWORKING_HOOKED.store(false, Ordering::SeqCst);
        STEAM_NETWORKING_HOOKING.store(false, Ordering::SeqCst);
    }
}
