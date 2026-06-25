//! Process discovery and Native Hook injection orchestration for Isaac.

mod error;
mod paths;
mod platform;
mod process;

pub use error::{InjectionStep, InjectorError};
pub use paths::{
    LEGACY_NATIVE_HOOK_DLL, LEGACY_NATIVE_INJECTOR_EXE, NATIVE_HOOK_DLL, NATIVE_INJECTOR_EXE,
    NativeHookPaths, injector_args, resolve_native_hook_paths, run_injector,
};
pub use platform::inject;
pub use process::{
    ISAAC_PROCESS_NAME, IsaacProcess, find_isaac_process, is_process_running, wait_for_isaac,
};
