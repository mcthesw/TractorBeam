//! Diagnostics helpers for collecting bridge logs and summaries.

/// Name of the hook telemetry file written by the native hook.
pub const HOOK_TRACE_FILE: &str = "eos_probe.jsonl";

/// Name of the bridge config file read by the native hook.
pub const BRIDGE_CONFIG_FILE: &str = "isaac_bridge_config.txt";

/// Returns the known diagnostic filenames that should be collected first.
pub fn primary_diagnostic_files() -> [&'static str; 2] {
    [HOOK_TRACE_FILE, BRIDGE_CONFIG_FILE]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exposes_primary_diagnostics() {
        assert_eq!(
            primary_diagnostic_files(),
            [HOOK_TRACE_FILE, BRIDGE_CONFIG_FILE]
        );
    }
}
