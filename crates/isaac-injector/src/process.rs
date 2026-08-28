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
    find_isaac_processes().into_iter().next()
}

#[must_use]
pub fn find_isaac_processes() -> Vec<IsaacProcess> {
    let mut system = System::new();
    system.refresh_processes(ProcessesToUpdate::All, true);
    let mut processes = system
        .processes()
        .values()
        .filter(|process| process_looks_like_isaac(process))
        .map(|process| IsaacProcess {
            pid: process.pid().as_u32(),
            name: process.name().to_string_lossy().into_owned(),
            started_at: process.start_time(),
        })
        .collect::<Vec<_>>();
    sort_newest_first(&mut processes);
    processes
}

fn process_looks_like_isaac(process: &sysinfo::Process) -> bool {
    let name = process.name().to_string_lossy();
    if name.eq_ignore_ascii_case(ISAAC_PROCESS_NAME) {
        return true;
    }
    process.cmd().iter().any(|argument| {
        argument
            .to_string_lossy()
            .to_ascii_lowercase()
            .contains(ISAAC_PROCESS_NAME)
    })
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

fn sort_newest_first(processes: &mut [IsaacProcess]) {
    processes.sort_by_key(|process| std::cmp::Reverse((process.started_at, process.pid)));
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
    fn process_name_needle_matches_proton_command_lines() {
        assert!(
            "Z:\\home\\user\\.steam\\steamapps\\common\\The Binding of Isaac Rebirth\\isaac-ng.exe"
                .to_ascii_lowercase()
                .contains(ISAAC_PROCESS_NAME)
        );
        assert!(!"wine64-preloader".eq_ignore_ascii_case(ISAAC_PROCESS_NAME));
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

    #[test]
    fn process_candidates_are_sorted_by_newest_start_then_pid() {
        let mut processes = vec![
            process(42, 100),
            process(7, 101),
            process(9, 101),
            process(100, 99),
        ];

        sort_newest_first(&mut processes);

        assert_eq!(
            processes
                .iter()
                .map(|process| (process.pid, process.started_at))
                .collect::<Vec<_>>(),
            [(9, 101), (7, 101), (42, 100), (100, 99)]
        );
    }

    fn process(pid: u32, started_at: u64) -> IsaacProcess {
        IsaacProcess {
            pid,
            name: ISAAC_PROCESS_NAME.to_owned(),
            started_at,
        }
    }
}
