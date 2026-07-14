use std::time::{Duration, Instant};

use super::*;
use crate::domain::DataSource;

fn state() -> RelayState {
    let config = RelayConfig {
        pow_difficulty_bits: 0,
        ..RelayConfig::default()
    };
    RelayState::new(config)
}

fn join(
    state: &mut RelayState,
    control_peer: PeerId,
    session: SessionKey,
    steam_id64: u64,
    profile: DataProfile,
    now: Instant,
) -> JoinReady {
    join_with_capabilities(
        state,
        control_peer,
        session,
        steam_id64,
        profile,
        tractor_beam_relay_protocol::CAP_ROOM_PATH_PROBE,
        now,
    )
}

fn join_with_capabilities(
    state: &mut RelayState,
    control_peer: PeerId,
    session: SessionKey,
    steam_id64: u64,
    profile: DataProfile,
    capabilities: u64,
    now: Instant,
) -> JoinReady {
    let challenge = state
        .begin_join(JoinBegin {
            control_peer,
            session,
            steam_id64,
            display_name: Some(format!("peer-{steam_id64}")),
            profile,
            capabilities,
            now,
        })
        .unwrap();
    state
        .complete_join(control_peer, challenge.challenge_id, "", now)
        .unwrap()
        .0
}

#[test]
fn probe_routing_requires_capability_and_selected_source_path() {
    let now = Instant::now();
    let session = SessionKey([11; 16]);
    let mut state = state();
    let sender = join(
        &mut state,
        PeerId::new(1),
        session,
        101,
        DataProfile::Tcp,
        now,
    );
    let _target = join_with_capabilities(
        &mut state,
        PeerId::new(2),
        session,
        202,
        DataProfile::Tcp,
        0,
        now,
    );
    let request = RouteProbe {
        connection_id: sender.connection_id,
        from_steam_id64: 101,
        to_steam_id64: 202,
        source: DataSource::Tcp(PeerId::new(1)),
        frame_bytes: tractor_beam_relay_protocol::PROBE_FRAME_HEADER_LEN,
        now,
    };
    assert_eq!(
        state.route_probe(request),
        Err(StateError::ProbeUnsupported)
    );

    let _target = join(
        &mut state,
        PeerId::new(3),
        session,
        202,
        DataProfile::Tcp,
        now,
    );
    assert_eq!(
        state.route_probe(request).unwrap(),
        DataDestination::Tcp(PeerId::new(3))
    );
    assert_eq!(
        state.route_probe(RouteProbe {
            source: DataSource::Udp("127.0.0.1:40000".parse().unwrap()),
            ..request
        }),
        Err(StateError::ProfileMismatch)
    );
}

#[test]
fn credential_scopes_room_without_exposing_a_name() {
    let now = Instant::now();
    let session = SessionKey([7; 16]);
    let mut state = state();
    let first = join(
        &mut state,
        PeerId::new(1),
        session,
        76_561_198_000_000_001,
        DataProfile::Tcp,
        now,
    );
    let second = join(
        &mut state,
        PeerId::new(2),
        session,
        76_561_198_000_000_002,
        DataProfile::Tcp,
        now,
    );

    assert_ne!(first.connection_id, second.connection_id);
    assert_eq!(state.room_count(), 1);
    assert_eq!(state.peer_count(), 2);
}

#[test]
fn detach_resume_preserves_connection_and_frame_window() {
    let now = Instant::now();
    let session = SessionKey([8; 16]);
    let mut state = state();
    let ready = join(
        &mut state,
        PeerId::new(1),
        session,
        101,
        DataProfile::Tcp,
        now,
    );
    let target = join(
        &mut state,
        PeerId::new(2),
        session,
        202,
        DataProfile::Tcp,
        now,
    );

    assert_eq!(
        state
            .route_data(RouteData {
                connection_id: ready.connection_id,
                frame_id: 1,
                from_steam_id64: 101,
                to_steam_id64: 202,
                source: DataSource::Tcp(PeerId::new(1)),
                frame_bytes: 128,
                now,
            })
            .unwrap(),
        DataDestination::Tcp(PeerId::new(2))
    );
    assert!(state.detach(PeerId::new(1), now).is_some());
    let resumed = state
        .resume(
            PeerId::new(3),
            ready.connection_id,
            ready.resume_key,
            now + Duration::from_secs(1),
        )
        .unwrap();
    assert_eq!(resumed.connection_id, ready.connection_id);
    assert_eq!(
        state.route_data(RouteData {
            connection_id: ready.connection_id,
            frame_id: 1,
            from_steam_id64: 101,
            to_steam_id64: 202,
            source: DataSource::Tcp(PeerId::new(3)),
            frame_bytes: 128,
            now: now + Duration::from_secs(1),
        }),
        Err(StateError::DuplicateFrame)
    );
    assert_eq!(
        state.control_peer(target.connection_id),
        Some(PeerId::new(2))
    );
}

#[test]
fn grace_expiry_removes_detached_peer_once() {
    let now = Instant::now();
    let mut state = state();
    let ready = join(
        &mut state,
        PeerId::new(1),
        SessionKey([9; 16]),
        101,
        DataProfile::Tcp,
        now,
    );
    let _ = state.detach(PeerId::new(1), now);

    state.cleanup(now + Duration::from_secs(119));
    assert_eq!(state.peer_count(), 1);
    state.cleanup(now + Duration::from_secs(120));
    assert_eq!(state.peer_count(), 0);
    assert!(matches!(
        state.resume(
            PeerId::new(2),
            ready.connection_id,
            ready.resume_key,
            now + Duration::from_secs(121),
        ),
        Err(ResumeFailure::UnknownConnection)
    ));
}

#[test]
fn udp_profile_requires_bound_source_tuple() {
    let now = Instant::now();
    let session = SessionKey([10; 16]);
    let address = "127.0.0.1:40000".parse().unwrap();
    let mut state = state();
    let sender = join(
        &mut state,
        PeerId::new(1),
        session,
        101,
        DataProfile::Udp,
        now,
    );
    let _target = join(
        &mut state,
        PeerId::new(2),
        session,
        202,
        DataProfile::Tcp,
        now,
    );

    assert_eq!(
        state.route_data(RouteData {
            connection_id: sender.connection_id,
            frame_id: 1,
            from_steam_id64: 101,
            to_steam_id64: 202,
            source: DataSource::Udp(address),
            frame_bytes: 128,
            now,
        }),
        Err(StateError::PathNotValidated)
    );
    state
        .bind_udp_path(
            sender.connection_id,
            state.path_key(sender.connection_id).unwrap(),
            address,
            now,
        )
        .unwrap();
    assert_eq!(
        state
            .route_data(RouteData {
                connection_id: sender.connection_id,
                frame_id: 1,
                from_steam_id64: 101,
                to_steam_id64: 202,
                source: DataSource::Udp(address),
                frame_bytes: 128,
                now,
            })
            .unwrap(),
        DataDestination::Tcp(PeerId::new(2))
    );
}
