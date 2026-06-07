//! Hook injection orchestration for Isaac.

/// Process image name used by the current Windows build.
pub const ISAAC_PROCESS_NAME: &str = "isaac-ng.exe";

/// Native hook DLL name produced by the C++ hook build.
pub const NATIVE_HOOK_DLL: &str = "isaac_eos_probe.dll";

/// Native injector executable name produced by the C++ hook build.
pub const NATIVE_INJECTOR_EXE: &str = "eos_probe_injector.exe";

/// Minimal command line shape for the current native injector.
pub fn injector_args(pid: u32, dll_path: &str) -> [String; 4] {
    [
        "--pid".to_owned(),
        pid.to_string(),
        "--dll".to_owned(),
        dll_path.to_owned(),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_injector_args() {
        assert_eq!(
            injector_args(42, "hook.dll"),
            ["--pid", "42", "--dll", "hook.dll"]
        );
    }
}
