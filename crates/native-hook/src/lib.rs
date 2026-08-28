//! Native Hook DLL for the SteamNetworking006 packet path.
//!
//! The real hook target is 32-bit Windows because `isaac-ng.exe` is a 32-bit
//! process. On Linux + Proton the same DLL is deployed as `winmm.dll` beside
//! the game executable. Other targets compile a small marker module so the rest
//! of the workspace can be checked without a hook toolchain.

#[cfg(all(windows, target_arch = "x86"))]
mod windows;

#[cfg(all(windows, target_arch = "x86"))]
#[must_use]
pub const fn supported_target() -> bool {
    true
}

#[cfg(not(all(windows, target_arch = "x86")))]
#[must_use]
pub const fn supported_target() -> bool {
    false
}
