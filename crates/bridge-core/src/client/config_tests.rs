use super::*;

#[test]
fn parses_relay_presets_and_defaults() {
    let raw = r#"
default_transport = "tcp"
default_mode = "pure"
selected_relay = "current"
[session_health]
enabled = true
runtime_rtt_enabled = false
snapshot_interval_seconds = 10
[[relays]]
id = "current"
name = "Current test relay"
host = "relay.example.test"
port = 25910
udp = true
tcp = true
default_transport = "tcp"
"#;
    let config: ClientConfig = toml::from_str::<RawClientConfig>(raw)
        .unwrap()
        .try_into()
        .unwrap();
    assert_eq!(config.default_transport, TransportChoice::Tcp);
    assert_eq!(config.default_mode, SessionMode::Pure);
    assert!(config.session_health.enabled);
    assert!(!config.session_health.runtime_rtt_enabled);
    assert_eq!(config.session_health.snapshot_interval_seconds, 10);
    assert_eq!(config.selected_relay_index(), Some(0));
    assert_eq!(
        config.relays[0].preferred_transport(TransportChoice::Udp),
        TransportChoice::Tcp
    );
}

#[test]
fn rejects_invalid_session_health_interval() {
    let raw = "[session_health]\nenabled = true\nsnapshot_interval_seconds = 0\n";
    let error =
        ClientConfig::try_from(toml::from_str::<RawClientConfig>(raw).unwrap()).unwrap_err();
    assert!(matches!(error, ClientConfigError::InvalidSessionHealth(_)));
}

#[test]
fn defaults_transport_to_tcp_when_omitted() {
    let config: ClientConfig = toml::from_str::<RawClientConfig>("")
        .unwrap()
        .try_into()
        .unwrap();
    assert_eq!(config.default_transport, TransportChoice::Tcp);
}

#[test]
fn save_selection_writes_keys_without_clobbering_others() {
    let temp = tempfile::tempdir().unwrap();
    let dir = temp.path();
    let config_path = dir.join(CLIENT_CONFIG_FILE);
    std::fs::write(
        &config_path,
        "# keep this comment\ndefault_transport = \"tcp\"\nroom = \"legacy-room-value\"\n[[relays]]\nid = \"r1\"\nname = \"Relay 1\"\nhost = \"relay.example.test\"\nport = 25910\n",
    )
    .unwrap();
    save_client_config_selection_to(
        &config_path,
        &ClientConfigSelection {
            selected_relay: Some("r1".to_owned()),
            selected_steam_id64: Some("76561198000000001".to_owned()),
        },
    )
    .unwrap();
    let content = std::fs::read_to_string(&config_path).unwrap();
    assert!(content.contains("selected_relay = \"r1\""));
    assert!(content.contains("selected_steam_id64 = \"76561198000000001\""));
    assert!(content.contains("# keep this comment"));
    assert!(content.contains("room = \"legacy-room-value\""));
    assert!(content.contains("[[relays]]"));
}

#[test]
fn save_selection_reports_the_main_config_write_error() {
    let temp = tempfile::tempdir().unwrap();
    let dir = temp.path().join("config.toml");
    std::fs::create_dir_all(&dir).unwrap();

    let error = save_client_config_selection_to(&dir, &ClientConfigSelection::default())
        .expect_err("a directory cannot be replaced as config.toml");

    assert!(matches!(error, ClientConfigError::Io { .. }));
}
