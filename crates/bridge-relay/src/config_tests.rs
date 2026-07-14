use argh::FromArgs;

use super::{
    Args, DEFAULT_BYTE_RATE_LIMIT_BURST, DEFAULT_BYTE_RATE_LIMIT_PER_SECOND,
    DEFAULT_HEALTH_PONGS_PER_SECOND_PER_IP, DEFAULT_MAX_PACKET_SIZE, DEFAULT_RATE_LIMIT_PER_SECOND,
    RelayConfig,
};

#[test]
fn parses_command_line_options_with_argh() {
    let args = Args::from_args(
        &["tractor-beam-relay"],
        &[
            "--config",
            "relay.toml",
            "--bind",
            "127.0.0.1:25910",
            "--tcp-bind",
            "127.0.0.1:25911",
            "--disable-tcp",
        ],
    )
    .unwrap();

    assert_eq!(args.config, Some("relay.toml".into()));
    assert_eq!(args.bind.as_deref(), Some("127.0.0.1:25910"));
    assert_eq!(args.tcp_bind.as_deref(), Some("127.0.0.1:25911"));
    assert!(args.disable_tcp);
    assert!(!args.should_print_version());
}

#[test]
fn parses_short_and_long_version_switches() {
    for version_switch in ["-V", "--version"] {
        let args = Args::from_args(&["tractor-beam-relay"], &[version_switch]).unwrap();
        assert!(args.should_print_version());
    }
}

#[test]
fn parses_minimal_sectioned_config() {
    let config = RelayConfig::from_toml(
        r#"
[relay_server]
udp_bind = "0.0.0.0:25910"
tcp_bind = "0.0.0.0:25910"

[admission]
pow_difficulty_bits = 18

[room_limits]
max_rooms = 256

[traffic_limits]
rate_limit_per_second = 5000
byte_rate_limit_per_second = 8388608
byte_rate_limit_burst = 16777216

[access_control]
blocked_cidrs = ["203.0.113.10/32"]
"#,
    )
    .unwrap();

    assert_eq!(config.udp_bind.as_deref(), Some("0.0.0.0:25910"));
    assert_eq!(config.tcp_bind.as_deref(), Some("0.0.0.0:25910"));
    assert_eq!(config.pow_difficulty_bits, 18);
    assert_eq!(config.max_rooms, 256);
    assert_eq!(config.max_packet_size, DEFAULT_MAX_PACKET_SIZE);
    assert_eq!(config.rate_limit_per_second, DEFAULT_RATE_LIMIT_PER_SECOND);
    assert_eq!(
        config.byte_rate_limit_per_second,
        DEFAULT_BYTE_RATE_LIMIT_PER_SECOND
    );
    assert_eq!(config.byte_rate_limit_burst, DEFAULT_BYTE_RATE_LIMIT_BURST);
    assert_eq!(
        config.health_pongs_per_second_per_ip,
        DEFAULT_HEALTH_PONGS_PER_SECOND_PER_IP
    );
    assert_eq!(config.blocked_cidrs.len(), 1);
}

#[test]
fn listener_bind_presence_controls_transports() {
    let tcp_only = RelayConfig::from_toml(
        r#"
[relay_server]
tcp_bind = "127.0.0.1:25910"
"#,
    )
    .unwrap();
    assert_eq!(tcp_only.udp_bind, None);
    assert_eq!(tcp_only.tcp_bind.as_deref(), Some("127.0.0.1:25910"));

    let udp_only = RelayConfig::from_toml(
        r#"
[relay_server]
udp_bind = "127.0.0.1:25910"
"#,
    )
    .unwrap_err();
    assert!(udp_only.to_string().contains("tcp_bind is required"));
}

#[test]
fn rejects_legacy_flat_config() {
    let error = RelayConfig::from_toml(
        r#"
bind = "127.0.0.1:25910"
tcp_enabled = false
max_packet_size = 1500
rate_limit_per_second = 240
max_rooms = 1024
blocked_cidrs = ["198.51.100.0/24"]
"#,
    )
    .unwrap_err();

    assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
    assert!(error.to_string().contains("unknown field"));
}

#[test]
fn traffic_limits_override_defaults() {
    let config = RelayConfig::from_toml(
        r#"
[relay_server]
tcp_bind = "127.0.0.1:25910"

[traffic_limits]
rate_limit_per_second = 240
byte_rate_limit_per_second = 204800
byte_rate_limit_burst = 409600
"#,
    )
    .unwrap();

    assert_eq!(config.rate_limit_per_second, 240);
    assert_eq!(config.byte_rate_limit_per_second, 204_800);
    assert_eq!(config.byte_rate_limit_burst, 409_600);
}

#[test]
fn traffic_limits_can_be_partially_overridden() {
    let config = RelayConfig::from_toml(
        r#"
[relay_server]
tcp_bind = "127.0.0.1:25910"

[traffic_limits]
byte_rate_limit_per_second = 1048576
"#,
    )
    .unwrap();

    assert_eq!(config.rate_limit_per_second, DEFAULT_RATE_LIMIT_PER_SECOND);
    assert_eq!(config.byte_rate_limit_per_second, 1_048_576);
    assert_eq!(config.byte_rate_limit_burst, DEFAULT_BYTE_RATE_LIMIT_BURST);
}

#[test]
fn rejects_zero_traffic_limits() {
    let error = RelayConfig::from_toml(
        r#"
[relay_server]
tcp_bind = "127.0.0.1:25910"

[traffic_limits]
byte_rate_limit_burst = 0
"#,
    )
    .unwrap_err();

    assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
    assert!(
        error
            .to_string()
            .contains("byte_rate_limit_burst must be greater than 0")
    );
}

#[test]
fn rejects_config_without_listeners() {
    let error = RelayConfig::from_toml(
        r#"
[relay_server]
"#,
    )
    .unwrap_err();

    assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
    assert!(error.to_string().contains("tcp_bind is required"));
}

#[test]
fn telemetry_is_enabled_only_by_explicit_section() {
    let config = RelayConfig::from_toml(
        r#"
[relay_server]
tcp_bind = "127.0.0.1:25910"
"#,
    )
    .unwrap();
    assert!(config.telemetry.is_none());

    let config = RelayConfig::from_toml(
        r#"
[relay_server]
tcp_bind = "127.0.0.1:25910"

[telemetry]
otlp_endpoint = "http://127.0.0.1:4317"
service_instance_id = "relay-test-1"
"#,
    )
    .unwrap();
    let telemetry = config.telemetry.unwrap();
    assert_eq!(telemetry.otlp_endpoint, "http://127.0.0.1:4317");
    assert_eq!(telemetry.service_instance_id, "relay-test-1");
}

#[test]
fn rejects_invalid_telemetry_configuration() {
    for section in [
        r#"
[telemetry]
otlp_endpoint = ""
service_instance_id = "relay-test-1"
"#,
        r#"
[telemetry]
otlp_endpoint = "http://127.0.0.1:4317"
service_instance_id = ""
"#,
    ] {
        let error = RelayConfig::from_toml(&format!(
            "[relay_server]\ntcp_bind = \"127.0.0.1:25910\"\n{section}"
        ))
        .unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
    }
}

#[test]
fn rejects_obsolete_data_trace_sampling_configuration() {
    let error = RelayConfig::from_toml(
        r#"
[relay_server]
tcp_bind = "127.0.0.1:25910"

[telemetry]
otlp_endpoint = "http://127.0.0.1:4317"
service_instance_id = "relay-test-1"
data_trace_sample_ratio = 0.001
"#,
    )
    .unwrap_err();

    assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
    assert!(error.to_string().contains("data_trace_sample_ratio"));
}
