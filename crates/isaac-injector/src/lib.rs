//! Process discovery and Native Hook injection orchestration for Isaac.

mod error;
#[cfg(target_os = "linux")]
mod linux;
mod paths;
mod platform;
mod process;
mod report;

pub use error::{InjectionStep, InjectorError};
#[cfg(target_os = "linux")]
pub use linux::{
    ProtonWinmmSidecar, deploy_proton_winmm_sidecar, recover_stale_proton_winmm_sidecar,
    remove_proton_winmm_sidecar, verify_proton_sidecar_loaded,
};
pub use paths::{
    InjectionGuard, InjectorLaunchEvent, NATIVE_HOOK_DLL, NATIVE_INJECTOR_EXE, NativeHookPaths,
    injector_args, resolve_native_hook_paths, run_injector, run_injector_with_elevated_retry,
};
pub use platform::{inject, inject_guarded};
pub use process::{
    ISAAC_PROCESS_NAME, IsaacProcess, find_isaac_process, find_isaac_processes, is_process_running,
    wait_for_isaac,
};
pub use report::{read_failure_report, write_failure_report};
