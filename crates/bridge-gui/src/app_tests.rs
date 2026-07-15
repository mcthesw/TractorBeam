use super::*;

#[test]
fn lan_probe_results_require_choice_only_when_multiple_are_reachable() {
    assert_eq!(lan_probe_disposition(0), LanProbeDisposition::NoneReachable);
    assert_eq!(lan_probe_disposition(1), LanProbeDisposition::JoinOne);
    assert_eq!(lan_probe_disposition(2), LanProbeDisposition::Choose);
}

#[test]
fn lan_creation_selects_every_recommended_adapter_by_default() {
    let adapters = vec![LanAdapter {
        adapter_id: "7:test".to_owned(),
        name: "Virtual LAN".to_owned(),
        interface_index: 7,
        addresses: vec![tractor_beam_core::LanAdapterAddress {
            adapter_id: "7:test".to_owned(),
            name: "Virtual LAN".to_owned(),
            address: "10.10.0.2".parse().unwrap(),
            interface_index: 7,
        }],
    }];
    let selected = default_lan_adapter_selection(adapters);
    assert!(selected.iter().all(|(_, selected)| *selected));
}

#[test]
fn route_switch_requires_an_idle_client_without_a_lan_room() {
    assert!(route_switch_allowed(false, SessionStatus::Idle));
    assert!(!route_switch_allowed(true, SessionStatus::Idle));
    assert!(!route_switch_allowed(false, SessionStatus::Running));
}

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
