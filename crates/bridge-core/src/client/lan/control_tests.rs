use tractor_beam_direct_protocol::InstanceId;

use super::*;

fn loopback_adapter(id: u32) -> LanAdapterAddress {
    LanAdapterAddress {
        adapter_id: format!("test-{id}"),
        name: format!("Loopback {id}"),
        address: IpAddr::V4(std::net::Ipv4Addr::LOCALHOST),
        interface_index: id,
    }
}

fn identity(id: u8) -> PeerIdentity {
    PeerIdentity::new(u64::from(id), InstanceId::from_bytes([id; 16]))
}

async fn room(id: u8, credential: SessionCredential) -> LanControlPlane {
    LanControlPlane::create(
        identity(id),
        format!("Peer {id}"),
        credential,
        &[loopback_adapter(u32::from(id))],
    )
    .await
    .unwrap()
}

async fn wait_connected(room: &LanControlPlane, expected: usize) {
    time::timeout(Duration::from_secs(5), async {
        loop {
            let connected = room
                .peer_states()
                .iter()
                .filter(|peer| peer.connection == LanPeerConnectionState::Connected)
                .count();
            if connected == expected {
                return;
            }
            time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn invitation_is_created_only_after_tcp_and_udp_bind() {
    let credential = SessionCredential::from_bytes([7; 16]);
    let room = LanControlPlane::create(
        identity(1),
        "Alice".to_owned(),
        credential,
        &[loopback_adapter(1)],
    )
    .await
    .unwrap();
    let invitation = room.invitation();

    assert_eq!(invitation.introducer, identity(1));
    assert_eq!(invitation.control_endpoints, room.control_endpoints());
    assert_ne!(invitation.control_endpoints[0].port(), 0);
    room.shutdown().await;
}

#[tokio::test]
async fn probe_is_bounded_non_mutating_and_credential_scoped() {
    let credential = SessionCredential::from_bytes([7; 16]);
    let room = LanControlPlane::create(
        identity(1),
        "Alice".to_owned(),
        credential,
        &[loopback_adapter(1)],
    )
    .await
    .unwrap();
    let invitation = room.invitation();

    let results = LanControlPlane::probe(&invitation).await;
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].endpoint, invitation.control_endpoints[0]);
    assert_eq!(room.descriptor().identity, identity(1));

    let mut wrong = invitation;
    wrong.session_credential = SessionCredential::from_bytes([8; 16]);
    assert!(LanControlPlane::probe(&wrong).await.is_empty());
    room.shutdown().await;
}

#[tokio::test]
async fn probe_returns_zero_or_many_results_in_endpoint_order() {
    let credential = SessionCredential::from_bytes([7; 16]);
    let room = LanControlPlane::create(
        identity(1),
        "Alice".to_owned(),
        credential,
        &[
            loopback_adapter(1),
            LanAdapterAddress {
                adapter_id: "test-2".to_owned(),
                name: "Loopback 2".to_owned(),
                address: "127.0.0.2".parse().unwrap(),
                interface_index: 2,
            },
        ],
    )
    .await
    .unwrap();

    let results = LanControlPlane::probe(&room.invitation()).await;
    assert_eq!(results.len(), 2);
    assert!(results[0].endpoint < results[1].endpoint);

    let unreachable = LanJoinCode {
        introducer: identity(1),
        control_endpoints: vec!["127.0.0.1:1".parse().unwrap()],
        session_credential: credential,
    };
    assert!(LanControlPlane::probe(&unreachable).await.is_empty());
    room.shutdown().await;
}

#[tokio::test]
async fn concurrent_joiners_converge_to_direct_membership_mesh() {
    let credential = SessionCredential::from_bytes([9; 16]);
    let introducer = room(1, credential).await;
    let alice = room(2, credential).await;
    let bob = room(3, credential).await;
    let invitation = introducer.invitation();
    let endpoint = invitation.control_endpoints[0];

    let (alice_join, bob_join) = tokio::join!(
        alice.join(&invitation, endpoint),
        bob.join(&invitation, endpoint),
    );
    alice_join.unwrap();
    bob_join.unwrap();
    wait_connected(&introducer, 2).await;
    wait_connected(&alice, 2).await;
    wait_connected(&bob, 2).await;

    introducer.shutdown().await;
    time::sleep(Duration::from_millis(100)).await;
    assert!(alice.peer_states().iter().any(|state| {
        state.peer.identity == identity(3) && state.connection == LanPeerConnectionState::Connected
    }));
    assert!(bob.peer_states().iter().any(|state| {
        state.peer.identity == identity(2) && state.connection == LanPeerConnectionState::Connected
    }));
    alice.shutdown().await;
    bob.shutdown().await;
}

#[tokio::test]
async fn simultaneous_duplicate_dials_keep_one_link_per_peer() {
    let credential = SessionCredential::from_bytes([10; 16]);
    let alice = room(1, credential).await;
    let bob = room(2, credential).await;
    let alice_invitation = alice.invitation();
    let bob_invitation = bob.invitation();

    let (left, right) = tokio::join!(
        alice.join(&bob_invitation, bob_invitation.control_endpoints[0]),
        bob.join(&alice_invitation, alice_invitation.control_endpoints[0]),
    );
    left.unwrap();
    right.unwrap();
    wait_connected(&alice, 1).await;
    wait_connected(&bob, 1).await;
    assert_eq!(
        alice
            .peer_states()
            .iter()
            .filter(|state| state.connection == LanPeerConnectionState::Connected)
            .count(),
        1
    );
    assert_eq!(
        bob.peer_states()
            .iter()
            .filter(|state| state.connection == LanPeerConnectionState::Connected)
            .count(),
        1
    );
    alice.shutdown().await;
    bob.shutdown().await;
}

#[tokio::test]
async fn abrupt_pair_loss_recovers_without_restarting_room() {
    let credential = SessionCredential::from_bytes([11; 16]);
    let alice = room(1, credential).await;
    let bob = room(2, credential).await;
    let invitation = alice.invitation();
    bob.join(&invitation, invitation.control_endpoints[0])
        .await
        .unwrap();
    wait_connected(&alice, 1).await;
    wait_connected(&bob, 1).await;

    let original = alice.test_link_id(identity(2)).unwrap();
    alice.test_interrupt_peer(identity(2));
    time::timeout(Duration::from_secs(5), async {
        loop {
            if alice
                .test_link_id(identity(2))
                .is_some_and(|id| id != original)
                && bob
                    .test_link_id(identity(1))
                    .is_some_and(|id| id != original)
            {
                break;
            }
            time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .unwrap();
    wait_connected(&alice, 1).await;
    wait_connected(&bob, 1).await;
    alice.shutdown().await;
    bob.shutdown().await;
}
