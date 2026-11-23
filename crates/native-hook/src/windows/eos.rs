//! The Steam build uses EOS P2P in some friend lobbies. Route at the actual
//! send/receive calls; the global Crossplay preference is not a lobby type.

use super::{bridge, iat};
use std::{
    ffi::{CStr, c_void},
    ptr,
    sync::{
        OnceLock,
        atomic::{AtomicBool, AtomicPtr, Ordering},
    },
};
use tractor_beam_hook_ipc::crossplay::{Packet, Reliability};
use windows_sys::Win32::System::LibraryLoader::{GetModuleHandleA, GetProcAddress};

type Handle = *mut c_void;
const OK: i32 = 0;
const NO_CONNECTION: i32 = 1;
const INVALID_PARAMETERS: i32 = 10;
const NOT_FOUND: i32 = 18;
const LIMIT_EXCEEDED: i32 = 22;
const STEAM: i32 = 1;
#[repr(C)]
struct Socket {
    version: i32,
    name: [u8; 33],
}
#[repr(C)]
struct Send {
    version: i32,
    local: Handle,
    remote: Handle,
    socket: *const Socket,
    channel: u8,
    len: u32,
    data: *const u8,
    delayed: i32,
    reliability: i32,
    disable_auto_accept: i32,
}
#[repr(C)]
struct Peek {
    version: i32,
    local: Handle,
    channel: *const u8,
}
#[repr(C)]
struct Receive {
    version: i32,
    local: Handle,
    max: u32,
    channel: *const u8,
}
#[repr(C)]
struct Mapping {
    version: i32,
    local: Handle,
    kind: i32,
    target: *const c_void,
}

type TickFn = unsafe extern "system" fn(Handle);
struct Sdk {
    tick: TickFn,
    connect: unsafe extern "system" fn(Handle) -> Handle,
    steam_id: unsafe extern "system" fn(Handle, *const Mapping, *mut u8, *mut i32) -> i32,
    product_id: unsafe extern "system" fn(Handle, *const Mapping) -> Handle,
}
static SDK: OnceLock<Sdk> = OnceLock::new();
static PLATFORM: AtomicPtr<c_void> = AtomicPtr::new(ptr::null_mut());
static CONNECT: AtomicPtr<c_void> = AtomicPtr::new(ptr::null_mut());
static SEND_SEEN: AtomicBool = AtomicBool::new(false);
static MAPPING_MISS: AtomicBool = AtomicBool::new(false);
static BRIDGED: AtomicBool = AtomicBool::new(false);
static DELIVERED: AtomicBool = AtomicBool::new(false);

pub(super) unsafe fn install() -> bool {
    let module = unsafe { GetModuleHandleA(c"EOSSDK-Win32-Shipping.dll".as_ptr().cast()) };
    if module.is_null() {
        // Steam-only builds do not import EOS.
        return true;
    }
    macro_rules! export {
        ($name:literal, $ty:ty) => {{
            let Some(f) = (unsafe { GetProcAddress(module, concat!($name, "\0").as_ptr()) }) else {
                return false;
            };
            unsafe { std::mem::transmute::<unsafe extern "system" fn() -> isize, $ty>(f) }
        }};
    }
    let sdk = Sdk {
        tick: export!("_EOS_Platform_Tick@4", TickFn),
        connect: export!(
            "_EOS_Platform_GetConnectInterface@4",
            unsafe extern "system" fn(Handle) -> Handle
        ),
        steam_id: export!(
            "_EOS_Connect_GetProductUserIdMapping@16",
            unsafe extern "system" fn(Handle, *const Mapping, *mut u8, *mut i32) -> i32
        ),
        product_id: export!(
            "_EOS_Connect_GetExternalAccountMapping@8",
            unsafe extern "system" fn(Handle, *const Mapping) -> Handle
        ),
    };
    if SDK.set(sdk).is_err() {
        return false;
    }
    macro_rules! entry {
        ($symbol:literal,$f:ident) => {{
            static ORIGINAL: AtomicPtr<c_void> = AtomicPtr::new(ptr::null_mut());
            iat::ImportPatch {
                symbol: $symbol,
                replacement: $f as *mut c_void,
                original: &ORIGINAL,
            }
        }};
    }
    let patched = unsafe {
        iat::patch_imports(
            GetModuleHandleA(ptr::null()),
            "EOSSDK-Win32-Shipping.dll",
            &[
                entry!("_EOS_Platform_Tick@4", tick),
                entry!("_EOS_P2P_SendPacket@8", send),
                entry!("_EOS_P2P_GetNextReceivedPacketSize@12", peek),
                entry!("_EOS_P2P_ReceivePacket@28", receive),
            ],
        )
    };
    bridge::log_info(format!("crossplay_p2p_available={patched}"));
    patched
}

unsafe extern "system" fn tick(platform: Handle) {
    let sdk = SDK.get().unwrap();
    if PLATFORM.load(Ordering::Relaxed) != platform {
        CONNECT.store(unsafe { (sdk.connect)(platform) }, Ordering::Release);
        PLATFORM.store(platform, Ordering::Relaxed);
    }
    unsafe { (sdk.tick)(platform) };
}

impl Sdk {
    fn steam_peer(&self, local: Handle, remote: Handle) -> Option<u64> {
        let connect = CONNECT.load(Ordering::Acquire);
        if connect.is_null() {
            return None;
        }
        let mut external = [0; 257];
        let mut n = external.len() as i32;
        let options = Mapping {
            version: 1,
            local,
            kind: STEAM,
            target: remote,
        };
        let result = unsafe { (self.steam_id)(connect, &options, external.as_mut_ptr(), &mut n) };
        if result != OK {
            if !MAPPING_MISS.swap(true, Ordering::Relaxed) {
                bridge::log_warn(format!(
                    "crossplay_steam_mapping_unavailable result={result}"
                ));
            }
            return None;
        }
        CStr::from_bytes_until_nul(&external)
            .ok()?
            .to_str()
            .ok()?
            .parse()
            .ok()
    }
    fn product_peer(&self, local: Handle, steam: u64) -> Option<Handle> {
        let connect = CONNECT.load(Ordering::Acquire);
        if connect.is_null() {
            return None;
        }
        let id = format!("{steam}\0");
        let options = Mapping {
            version: 1,
            local,
            kind: STEAM,
            target: id.as_ptr().cast(),
        };
        let remote = unsafe { (self.product_id)(connect, &options) };
        (!remote.is_null()).then_some(remote)
    }
}

// These functions are installed only for TB gameplay. They intentionally have
// no native P2P function to call when the destination or path is unavailable.
unsafe extern "system" fn send(_: Handle, options: *const Send) -> i32 {
    if !SEND_SEEN.swap(true, Ordering::Relaxed) {
        bridge::log_info("crossplay_p2p_send_observed");
    }
    if options.is_null() || unsafe { (*options).version } != 3 {
        return INVALID_PARAMETERS;
    }
    let o = unsafe { &*options };
    if o.socket.is_null() || unsafe { (*o.socket).version } != 1 || (o.len != 0 && o.data.is_null())
    {
        return INVALID_PARAMETERS;
    }
    if o.len as usize > tractor_beam_hook_ipc::crossplay::MAX_PACKET {
        return LIMIT_EXCEEDED;
    }
    let Some(peer) = SDK.get().unwrap().steam_peer(o.local, o.remote) else {
        return NO_CONNECTION;
    };
    let Some(socket) = CStr::from_bytes_until_nul(unsafe { &(*o.socket).name })
        .ok()
        .and_then(|s| s.to_str().ok())
    else {
        return INVALID_PARAMETERS;
    };
    let data = if o.len == 0 {
        &[]
    } else {
        unsafe { std::slice::from_raw_parts(o.data, o.len as usize) }
    };
    let reliability = match o.reliability {
        0 => Reliability::UnreliableUnordered,
        1 => Reliability::ReliableUnordered,
        2 => Reliability::ReliableOrdered,
        _ => return INVALID_PARAMETERS,
    };
    let Some(packet) = (Packet {
        socket,
        channel: o.channel,
        reliability,
        data,
    })
    .encode(peer, 0) else {
        return INVALID_PARAMETERS;
    };
    if !bridge::enqueue(packet) {
        return LIMIT_EXCEEDED;
    }
    if !BRIDGED.swap(true, Ordering::Relaxed) {
        bridge::log_info("crossplay_p2p_first_relay_send");
    }
    OK
}

unsafe extern "system" fn peek(_: Handle, o: *const Peek, size: *mut u32) -> i32 {
    if o.is_null() || unsafe { (*o).version } != 2 || size.is_null() {
        return INVALID_PARAMETERS;
    }
    let o = unsafe { &*o };
    let len =
        bridge::with_crossplay_packet(unsafe { o.channel.as_ref() }.copied(), false, |peer, p| {
            SDK.get()
                .unwrap()
                .product_peer(o.local, peer)
                .map(|_| p.data.len() as u32)
        });
    let Some(len) = len else {
        return NOT_FOUND;
    };
    unsafe {
        *size = len;
    }
    OK
}

unsafe extern "system" fn receive(
    _: Handle,
    o: *const Receive,
    peer: *mut Handle,
    socket: *mut Socket,
    channel: *mut u8,
    data: *mut u8,
    written: *mut u32,
) -> i32 {
    if o.is_null()
        || unsafe { (*o).version } != 2
        || peer.is_null()
        || socket.is_null()
        || channel.is_null()
        || data.is_null()
        || written.is_null()
    {
        return INVALID_PARAMETERS;
    }
    let o = unsafe { &*o };
    if bridge::with_crossplay_packet(unsafe { o.channel.as_ref() }.copied(), true, |steam, p| {
        let remote = SDK.get().unwrap().product_peer(o.local, steam)?;
        let mut sock = Socket {
            version: 1,
            name: [0; 33],
        };
        sock.name[..p.socket.len()].copy_from_slice(p.socket.as_bytes());
        let len = (o.max as usize).min(p.data.len());
        unsafe {
            *peer = remote;
            *socket = sock;
            *channel = p.channel;
            ptr::copy_nonoverlapping(p.data.as_ptr(), data, len);
            *written = len as u32;
        }
        Some(())
    })
    .is_some()
    {
        if !DELIVERED.swap(true, Ordering::Relaxed) {
            bridge::log_info("crossplay_p2p_first_relay_receive");
        }
        return OK;
    }
    NOT_FOUND
}
