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
fn parses_ipv6_relay_preset() {
    let raw = r#"
[[relays]]
id = "ipv6"
name = "IPv6 relay"
host = "[2001:db8::10]"
port = 25910
"#;

    let config: ClientConfig = toml::from_str::<RawClientConfig>(raw)
        .unwrap()
        .try_into()
        .unwrap();
    assert_eq!(config.relays[0].endpoint.host, "2001:db8::10");
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

#[test]
fn relay_catalog_adds_and_selects_a_numeric_record_without_clobbering_config() {
    let temp = tempfile::tempdir().unwrap();
    let config_path = temp.path().join(CLIENT_CONFIG_FILE);
    std::fs::write(
        &config_path,
        "# keep this comment\ncustom_key = \"keep\"\n[[relays]]\nid = \"legacy\"\nname = \"Legacy Relay\"\nhost = \"legacy.example.test\"\nport = 25910\n[[relays]]\nid = \"4\"\nname = \"Relay 4\"\nhost = \"four.example.test\"\nport = 25910\n",
    )
    .unwrap();

    let loaded = save_client_relay_catalog_to(
        &config_path,
        &RelayCatalogChange::Add(relay_input("New Relay", "new.example.test")),
    )
    .unwrap();

    assert_eq!(loaded.config.selected_relay.as_deref(), Some("5"));
    assert_eq!(loaded.config.relays.last().unwrap().id, "5");
    let content = std::fs::read_to_string(&config_path).unwrap();
    assert!(content.contains("# keep this comment"));
    assert!(content.contains("custom_key = \"keep\""));
    assert!(content.contains("next_relay_id = 6"));
}

#[test]
fn relay_catalog_updates_in_place_and_preserves_the_record_id() {
    let temp = tempfile::tempdir().unwrap();
    let config_path = temp.path().join(CLIENT_CONFIG_FILE);
    std::fs::write(
        &config_path,
        "selected_relay = \"relay-a\"\n[[relays]]\nid = \" relay-a \"\n# keep relay note\nname = \"Old\"\nhost = \"old.example.test\"\nport = 25910\n",
    )
    .unwrap();
    let mut input = relay_input("Updated", "new.example.test");
    input.supports_udp = false;

    let loaded = save_client_relay_catalog_to(
        &config_path,
        &RelayCatalogChange::Update {
            id: "relay-a".to_owned(),
            relay: input,
        },
    )
    .unwrap();

    assert_eq!(loaded.config.selected_relay.as_deref(), Some("relay-a"));
    assert_eq!(loaded.config.relays[0].id, "relay-a");
    assert_eq!(loaded.config.relays[0].name, "Updated");
    assert!(!loaded.config.relays[0].supports_udp);
    let content = std::fs::read_to_string(&config_path).unwrap();
    assert!(content.contains("# keep relay note"));
    assert!(content.contains("id = \" relay-a \""));
}

#[test]
fn relay_catalog_deletes_an_existing_id_with_surrounding_whitespace() {
    let temp = tempfile::tempdir().unwrap();
    let config_path = temp.path().join(CLIENT_CONFIG_FILE);
    std::fs::write(
        &config_path,
        "selected_relay = \"relay-a\"\n[[relays]]\nid = \" relay-a \"\nname = \"Relay A\"\nhost = \"relay.example.test\"\nport = 25910\n",
    )
    .unwrap();

    let loaded = save_client_relay_catalog_to(
        &config_path,
        &RelayCatalogChange::Delete {
            id: "relay-a".to_owned(),
        },
    )
    .unwrap();

    assert!(loaded.config.relays.is_empty());
    assert!(loaded.config.selected_relay.is_none());
}

#[test]
fn relay_catalog_delete_clears_selection_and_does_not_reuse_its_record_number() {
    let temp = tempfile::tempdir().unwrap();
    let config_path = temp.path().join(CLIENT_CONFIG_FILE);
    std::fs::write(
        &config_path,
        "selected_relay = \"8\"\n[[relays]]\nid = \"8\"\nname = \"Relay 8\"\nhost = \"eight.example.test\"\nport = 25910\n",
    )
    .unwrap();

    let deleted = save_client_relay_catalog_to(
        &config_path,
        &RelayCatalogChange::Delete { id: "8".to_owned() },
    )
    .unwrap();
    assert!(deleted.config.selected_relay.is_none());
    assert!(deleted.config.relays.is_empty());

    let added = save_client_relay_catalog_to(
        &config_path,
        &RelayCatalogChange::Add(relay_input("Relay 9", "nine.example.test")),
    )
    .unwrap();
    assert_eq!(added.config.selected_relay.as_deref(), Some("9"));
    assert_eq!(added.config.relays[0].id, "9");
}

#[test]
fn relay_catalog_rejects_an_unsupported_default_transport_without_writing() {
    let temp = tempfile::tempdir().unwrap();
    let config_path = temp.path().join(CLIENT_CONFIG_FILE);
    std::fs::write(&config_path, "# unchanged\n").unwrap();
    let mut input = relay_input("Broken", "broken.example.test");
    input.supports_tcp = false;

    let error =
        save_client_relay_catalog_to(&config_path, &RelayCatalogChange::Add(input)).unwrap_err();

    assert!(matches!(error, ClientConfigError::InvalidRelay(_)));
    assert_eq!(
        std::fs::read_to_string(config_path).unwrap(),
        "# unchanged\n"
    );
}

fn relay_input(name: &str, host: &str) -> RelayProfileInput {
    RelayProfileInput {
        name: name.to_owned(),
        endpoint: RelayEndpoint::new(host, 25910),
        supports_udp: true,
        supports_tcp: true,
        default_transport: TransportChoice::Tcp,
    }
}
