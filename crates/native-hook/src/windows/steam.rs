use std::{
    ffi::{CStr, c_char, c_void},
    ptr,
    sync::atomic::{AtomicBool, AtomicPtr, AtomicU32, Ordering},
};

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
static STEAM_NETWORKING_HOOKED: AtomicBool = AtomicBool::new(false);
static SEND_CALLS: AtomicU32 = AtomicU32::new(0);
static AVAILABLE_CALLS: AtomicU32 = AtomicU32::new(0);
static READ_CALLS: AtomicU32 = AtomicU32::new(0);
static SESSION_STATE_CALLS: AtomicU32 = AtomicU32::new(0);
static RUN_CALLBACK_CALLS: AtomicU32 = AtomicU32::new(0);

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
}

unsafe fn install_steam_networking_hooks(interface: *mut c_void) {
    if interface.is_null() || STEAM_NETWORKING_HOOKED.swap(true, Ordering::SeqCst) {
        return;
    }

    bridge::log_info("steam_networking006_hooking");
    let vtable = unsafe { *(interface.cast::<*mut *mut c_void>()) };
    unsafe {
        patch_vtable_slot(
            vtable,
            0,
            hook_send_p2p_packet as *mut c_void,
            &ORIGINAL_SEND,
        );
        patch_vtable_slot(
            vtable,
            1,
            hook_is_p2p_packet_available as *mut c_void,
            &ORIGINAL_AVAILABLE,
        );
        patch_vtable_slot(
            vtable,
            2,
            hook_read_p2p_packet as *mut c_void,
            &ORIGINAL_READ,
        );
        patch_vtable_slot(
            vtable,
            6,
            hook_get_p2p_session_state as *mut c_void,
            &ORIGINAL_SESSION_STATE,
        );
    }
    bridge::log_info("steam_networking006_hooked");
}

unsafe fn install_existing_steam_networking_interface(steam_module: HMODULE) {
    if steam_module.is_null() {
        bridge::log_warn("steam_probe module_missing=steam_api.dll");
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
) {
    let slot = unsafe { vtable.add(index) };
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
        return;
    }

    let current = unsafe { *slot };
    let _ = original.compare_exchange(ptr::null_mut(), current, Ordering::SeqCst, Ordering::SeqCst);
    unsafe {
        *slot = replacement;
    }

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
    let send_call = SEND_CALLS.fetch_add(1, Ordering::Relaxed) + 1;
    let bridged = bridge::send_packet(remote, data.cast(), bytes, send_type, channel);
    let result = if bridge::mode() == bridge::BridgeMode::Replace {
        bridged
    } else if let Some(original) = original_fn::<SteamSendP2PPacketFn>(&ORIGINAL_SEND) {
        unsafe { original(this, remote, data, bytes, send_type, channel) }
    } else {
        false
    };
    if should_sample(send_call) || !result {
        let level = if result {
            bridge::HookLogLevel::Debug
        } else {
            bridge::HookLogLevel::Warn
        };
        bridge::log(
            level,
            format!(
                "steam_send call={send_call} remote={remote} channel={channel} send_type={send_type} bytes={bytes} bridged={bridged} result={result}"
            ),
        );
    }
    result
}

unsafe extern "thiscall" fn hook_is_p2p_packet_available(
    this: *mut c_void,
    bytes: *mut u32,
    channel: i32,
) -> bool {
    let available_call = AVAILABLE_CALLS.fetch_add(1, Ordering::Relaxed) + 1;
    if bridge::has_packet(channel, bytes) {
        if should_sample(available_call) {
            bridge::log_debug(format!(
                "steam_available call={available_call} channel={channel} bridge_hit=true bytes={}",
                pointed_u32(bytes)
            ));
        }
        return true;
    }
    if (bridge::mode() != bridge::BridgeMode::Replace || bridge::should_fallback_to_steam())
        && let Some(original) = original_fn::<SteamIsP2PPacketAvailableFn>(&ORIGINAL_AVAILABLE)
    {
        let result = unsafe { original(this, bytes, channel) };
        if result || should_sample(available_call) {
            let level = if result {
                bridge::HookLogLevel::Debug
            } else {
                bridge::HookLogLevel::Trace
            };
            bridge::log(
                level,
                format!(
                    "steam_available call={available_call} channel={channel} bridge_hit=false steam_result={result} bytes={}",
                    pointed_u32(bytes)
                ),
            );
        }
        return result;
    }
    if should_sample(available_call) {
        bridge::log_trace(format!(
            "steam_available call={available_call} channel={channel} bridge_hit=false steam_result=false bytes={}",
            pointed_u32(bytes)
        ));
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
    let read_call = READ_CALLS.fetch_add(1, Ordering::Relaxed) + 1;
    if bridge::read_packet(channel, destination.cast(), max_bytes, bytes_read, remote) {
        if should_sample(read_call) {
            bridge::log_debug(format!(
                "steam_read call={read_call} channel={channel} bridge_hit=true peer={} bytes={} max_bytes={max_bytes}",
                pointed_u64(remote),
                pointed_u32(bytes_read)
            ));
        }
        return true;
    }
    if (bridge::mode() != bridge::BridgeMode::Replace || bridge::should_fallback_to_steam())
        && let Some(original) = original_fn::<SteamReadP2PPacketFn>(&ORIGINAL_READ)
    {
        let result = unsafe { original(this, destination, max_bytes, bytes_read, remote, channel) };
        if result || should_sample(read_call) {
            let level = if result {
                bridge::HookLogLevel::Debug
            } else {
                bridge::HookLogLevel::Trace
            };
            bridge::log(
                level,
                format!(
                    "steam_read call={read_call} channel={channel} bridge_hit=false steam_result={result} peer={} bytes={} max_bytes={max_bytes}",
                    pointed_u64(remote),
                    pointed_u32(bytes_read)
                ),
            );
        }
        return result;
    }
    if should_sample(read_call) {
        bridge::log_trace(format!(
            "steam_read call={read_call} channel={channel} bridge_hit=false steam_result=false max_bytes={max_bytes}"
        ));
    }
    false
}

fn should_sample(call: u32) -> bool {
    call <= 32 || call.is_multiple_of(1000)
}

fn should_sample_callback(call: u32) -> bool {
    call <= 8 || call.is_multiple_of(5_000)
}

fn pointed_u32(pointer: *const u32) -> u32 {
    if pointer.is_null() {
        0
    } else {
        unsafe { *pointer }
    }
}

fn pointed_u64(pointer: *const u64) -> u64 {
    if pointer.is_null() {
        0
    } else {
        unsafe { *pointer }
    }
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
