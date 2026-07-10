use super::*;

fn account(steam_id64: &str, most_recent: bool) -> SteamIdentity {
    SteamIdentity {
        steam_id64: steam_id64.to_owned(),
        display_name: format!("User {steam_id64}"),
        most_recent,
    }
}

#[test]
fn initial_selection_prefers_saved_account() {
    let accounts = [
        account("76561198000000001", true),
        account("76561198000000002", false),
    ];

    let selected = initial_selected_account(&accounts, Some("76561198000000002"));

    assert_eq!(selected, Some(1));
}

#[test]
fn initial_selection_uses_most_recent_without_saved_match() {
    let accounts = [
        account("76561198000000001", false),
        account("76561198000000002", true),
    ];

    let selected = initial_selected_account(&accounts, Some("76561198000000003"));

    assert_eq!(selected, Some(1));
}

#[test]
fn initial_selection_falls_back_to_first_account() {
    let accounts = [
        account("76561198000000001", false),
        account("76561198000000002", false),
    ];

    let selected = initial_selected_account(&accounts, None);

    assert_eq!(selected, Some(0));
}

#[test]
fn initial_selection_handles_empty_accounts() {
    let selected = initial_selected_account(&[], None);

    assert_eq!(selected, None);
}

#[test]
fn repeated_shutdown_requests_keep_the_original_deadline() {
    let now = Instant::now();
    let mut shutdown = ShutdownGate::default();

    assert!(shutdown.request(now));
    let deadline = shutdown.deadline;
    assert!(!shutdown.request(now + Duration::from_secs(2)));

    assert_eq!(shutdown.deadline, deadline);
}

#[test]
fn shutdown_gate_times_out_after_three_seconds() {
    let now = Instant::now();
    let mut shutdown = ShutdownGate::default();
    shutdown.request(now);

    assert!(!shutdown.timed_out(now + SHUTDOWN_DEADLINE - Duration::from_millis(1)));
    assert!(shutdown.timed_out(now + SHUTDOWN_DEADLINE));
}
