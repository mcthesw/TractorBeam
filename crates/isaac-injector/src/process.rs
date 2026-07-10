use std::{
    thread,
    time::{Duration, Instant},
};

use sysinfo::{ProcessesToUpdate, System};

use crate::InjectorError;

/// Process image name used by the current Windows Steam build.
pub const ISAAC_PROCESS_NAME: &str = "isaac-ng.exe";

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IsaacProcess {
    pub pid: u32,
    pub name: String,
    pub started_at: u64,
}

#[must_use]
pub fn find_isaac_process() -> Option<IsaacProcess> {
    find_process_by_name(ISAAC_PROCESS_NAME)
}

#[must_use]
pub fn is_process_running(expected: &IsaacProcess) -> bool {
    let mut system = System::new();
    system.refresh_processes(ProcessesToUpdate::All, true);
    system.processes().values().any(|process| {
        process_identity_matches(
            expected,
            process.pid().as_u32(),
            &process.name().to_string_lossy(),
            process.start_time(),
        )
    })
}

pub fn wait_for_isaac(
    timeout: Duration,
    poll_interval: Duration,
) -> Result<IsaacProcess, InjectorError> {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if let Some(process) = find_isaac_process() {
            return Ok(process);
        }
        thread::sleep(poll_interval);
    }
    Err(InjectorError::ProcessNotFound)
}

fn find_process_by_name(name: &str) -> Option<IsaacProcess> {
    let mut system = System::new();
    system.refresh_processes(ProcessesToUpdate::All, true);
    system
        .processes()
        .values()
        .find(|process| process.name().to_string_lossy().eq_ignore_ascii_case(name))
        .map(|process| IsaacProcess {
            pid: process.pid().as_u32(),
            name: process.name().to_string_lossy().into_owned(),
            started_at: process.start_time(),
        })
}

fn process_identity_matches(
    expected: &IsaacProcess,
    pid: u32,
    name: &str,
    started_at: u64,
) -> bool {
    expected.pid == pid
        && expected.started_at == started_at
        && expected.name.eq_ignore_ascii_case(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exposes_process_name() {
        assert_eq!(ISAAC_PROCESS_NAME, "isaac-ng.exe");
    }

    #[test]
    fn exact_process_identity_rejects_pid_reuse() {
        let expected = IsaacProcess {
            pid: 42,
            name: ISAAC_PROCESS_NAME.to_owned(),
            started_at: 100,
        };

        assert!(process_identity_matches(&expected, 42, "ISAAC-NG.EXE", 100));
        assert!(!process_identity_matches(
            &expected,
            42,
            ISAAC_PROCESS_NAME,
            101
        ));
    }
}
