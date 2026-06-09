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
}

#[must_use]
pub fn find_isaac_process() -> Option<IsaacProcess> {
    find_process_by_name(ISAAC_PROCESS_NAME)
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
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exposes_process_name() {
        assert_eq!(ISAAC_PROCESS_NAME, "isaac-ng.exe");
    }
}
