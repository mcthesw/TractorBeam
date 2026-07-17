use tokio::time;

use super::test_support::*;
use super::*;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn real_local_socket_roundtrips_game_control_and_shutdown() {
    let session = HookIpcSession::test();
    let (control_tx, control_rx) = control_channel();
    let (event_tx, mut event_rx) = tokio::sync::mpsc::channel(64);
    let cancellation = CancellationToken::new();
    let (mut from_hook, to_hook, worker) =
        start(session.clone(), control_rx, event_tx, cancellation.clone()).unwrap();
    let worker = tokio::spawn(worker);
    let expected_to_hook = packet(41, 7, b"client-to-hook");
    let fake_expected = expected_to_hook.clone();
    let fake = thread::spawn(move || {
        let mut stream = connect_fake_hook(&session);
        write_hook_message(
            &mut stream,
            &HookToClient::Game(packet(42, 8, b"hook-to-client")),
        )
        .unwrap();
        let mut decoder = FrameDecoder::new();
        let mut saw_game = false;
        let mut saw_control = false;
        loop {
            for message in read_client_messages(&mut stream, &mut decoder).unwrap() {
                match message {
                    ClientToHook::Game(packet) => {
                        assert_eq!(packet, fake_expected);
                        saw_game = true;
                    }
                    ClientToHook::InputDelay { id, command } => {
                        assert_eq!(command, InputDelayCommand::Read);
                        write_hook_message(
                            &mut stream,
                            &HookToClient::InputDelayResult { id, result: Ok(37) },
                        )
                        .unwrap();
                        saw_control = true;
                    }
                    ClientToHook::Ping { id } => {
                        write_hook_message(&mut stream, &HookToClient::Pong { id }).unwrap();
                    }
                    ClientToHook::Shutdown => {
                        assert!(saw_game);
                        assert!(saw_control);
                        return;
                    }
                    ClientToHook::Handshake(_) => panic!("duplicate client handshake"),
                }
            }
        }
    });

    let received = time::timeout(TEST_TIMEOUT, from_hook.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(received, packet(42, 8, b"hook-to-client"));
    assert!(to_hook.try_send(expected_to_hook));
    let control = tokio::task::spawn_blocking(move || {
        request_input_delay(&control_tx, 19, InputDelayCommand::Read)
    })
    .await
    .unwrap()
    .unwrap();
    assert_eq!(control, Ok(37));

    let connected = time::timeout(TEST_TIMEOUT, async {
        loop {
            if let Some(RuntimeEvent::HookIpc(state)) = event_rx.recv().await
                && state.connection == HookIpcConnectionState::Connected
            {
                return state;
            }
        }
    })
    .await
    .unwrap();
    assert_eq!(
        connected.negotiated_major,
        Some(tractor_beam_hook_ipc::PROTOCOL_MAJOR)
    );
    cancellation.cancel();
    time::timeout(TEST_TIMEOUT, worker)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    fake.join().unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn hook_goodbye_ends_worker_without_reconnect_timeout() {
    let session = HookIpcSession::test();
    let (_control_tx, control_rx) = control_channel();
    let (event_tx, mut event_rx) = tokio::sync::mpsc::channel(64);
    let cancellation = CancellationToken::new();
    let observed_cancellation = cancellation.clone();
    let (_from_hook, _to_hook, worker) =
        start(session.clone(), control_rx, event_tx, cancellation).unwrap();
    let worker = tokio::spawn(worker);
    let fake = thread::spawn(move || {
        let mut stream = connect_fake_hook(&session);
        write_hook_message(&mut stream, &HookToClient::Goodbye).unwrap();
    });

    time::timeout(TEST_TIMEOUT, worker)
        .await
        .expect("Hook Goodbye should end the IPC worker immediately")
        .unwrap()
        .unwrap();
    fake.join().unwrap();
    assert!(observed_cancellation.is_cancelled());

    while let Ok(event) = event_rx.try_recv() {
        assert!(!matches!(
            event,
            RuntimeEvent::HookIpc(state)
                if state.connection == HookIpcConnectionState::Failed
        ));
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn same_session_disconnect_reconnects_with_fresh_handshake() {
    let session = HookIpcSession::test();
    let (_control_tx, control_rx) = control_channel();
    let (event_tx, mut event_rx) = tokio::sync::mpsc::channel(64);
    let cancellation = CancellationToken::new();
    let (mut from_hook, _to_hook, worker) =
        start(session.clone(), control_rx, event_tx, cancellation.clone()).unwrap();
    let worker = tokio::spawn(worker);
    let fake = thread::spawn(move || {
        let mut first = connect_fake_hook(&session);
        write_hook_message(&mut first, &HookToClient::Game(packet(1, 1, b"first"))).unwrap();
        drop(first);
        let mut second = connect_fake_hook(&session);
        write_hook_message(&mut second, &HookToClient::Game(packet(2, 2, b"second"))).unwrap();
        wait_for_shutdown(&mut second);
    });

    assert_eq!(
        time::timeout(TEST_TIMEOUT, from_hook.recv())
            .await
            .unwrap()
            .unwrap(),
        packet(1, 1, b"first")
    );
    assert_eq!(
        time::timeout(TEST_TIMEOUT, from_hook.recv())
            .await
            .unwrap()
            .unwrap(),
        packet(2, 2, b"second")
    );
    let reconnected = time::timeout(TEST_TIMEOUT, async {
        loop {
            if let Some(RuntimeEvent::HookIpc(state)) = event_rx.recv().await
                && state.connection == HookIpcConnectionState::Connected
                && state.reconnects == 1
            {
                return;
            }
        }
    })
    .await;
    assert!(reconnected.is_ok());
    cancellation.cancel();
    time::timeout(TEST_TIMEOUT, worker)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    fake.join().unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn input_delay_control_is_not_starved_by_game_burst() {
    let session = HookIpcSession::test();
    let (control_tx, control_rx) = control_channel();
    let (event_tx, mut event_rx) = tokio::sync::mpsc::channel(64);
    let cancellation = CancellationToken::new();
    let (_from_hook, to_hook, worker) =
        start(session.clone(), control_rx, event_tx, cancellation.clone()).unwrap();
    let worker = tokio::spawn(worker);
    let fake = thread::spawn(move || {
        let mut stream = connect_fake_hook(&session);
        let mut decoder = FrameDecoder::new();
        loop {
            for message in read_client_messages(&mut stream, &mut decoder).unwrap() {
                match message {
                    ClientToHook::InputDelay { id, .. } => {
                        write_hook_message(
                            &mut stream,
                            &HookToClient::InputDelayResult { id, result: Ok(21) },
                        )
                        .unwrap();
                    }
                    ClientToHook::Ping { id } => {
                        write_hook_message(&mut stream, &HookToClient::Pong { id }).unwrap();
                    }
                    ClientToHook::Shutdown => return,
                    ClientToHook::Handshake(_) | ClientToHook::Game(_) => {}
                }
            }
        }
    });

    time::timeout(TEST_TIMEOUT, async {
        loop {
            if let Some(RuntimeEvent::HookIpc(state)) = event_rx.recv().await
                && state.connection == HookIpcConnectionState::Connected
            {
                return;
            }
        }
    })
    .await
    .expect("fake Hook should complete the local IPC handshake");

    let (response_tx, response_rx) = mpsc::sync_channel(1);
    control_tx
        .try_send(InputDelayCall {
            id: 77,
            command: InputDelayCommand::Read,
            response: response_tx,
        })
        .unwrap();
    for sequence in 0..(MAX_DATA_BURST as u32 * 2) {
        assert!(to_hook.try_send(packet(1, sequence, b"burst-payload")));
    }
    let result = tokio::task::spawn_blocking(move || response_rx.recv_timeout(TEST_TIMEOUT))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(result, Ok(21));

    cancellation.cancel();
    time::timeout(TEST_TIMEOUT, worker)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    fake.join().unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn real_local_socket_survives_temporary_write_backpressure() {
    const PACKET_COUNT: u32 = 64;
    const PAYLOAD_BYTES: usize = 4_096;
    let session = HookIpcSession::test();
    let (_control_tx, control_rx) = control_channel();
    let (event_tx, mut event_rx) = tokio::sync::mpsc::channel(64);
    let cancellation = CancellationToken::new();
    let (_from_hook, to_hook, worker) =
        start(session.clone(), control_rx, event_tx, cancellation.clone()).unwrap();
    let worker = tokio::spawn(worker);
    let (connected_tx, connected_rx) = mpsc::sync_channel(1);
    let (progress_tx, progress_rx) = mpsc::channel();
    let fake = thread::spawn(move || {
        let mut stream = connect_fake_hook(&session);
        connected_tx.send(()).unwrap();
        thread::sleep(Duration::from_millis(100));
        stream.set_nonblocking(false).unwrap();
        let mut decoder = FrameDecoder::new();
        let mut received = 0_u32;
        while received < PACKET_COUNT {
            for message in read_client_messages(&mut stream, &mut decoder).unwrap() {
                match message {
                    ClientToHook::Game(packet) => {
                        assert_eq!(packet.sequence, received);
                        assert_eq!(packet.payload.len(), PAYLOAD_BYTES);
                        received = received.saturating_add(1);
                        progress_tx.send(received).unwrap();
                    }
                    ClientToHook::Ping { id } => {
                        write_hook_message(&mut stream, &HookToClient::Pong { id }).unwrap();
                    }
                    ClientToHook::Handshake(_) => panic!("duplicate client handshake"),
                    ClientToHook::InputDelay { .. } | ClientToHook::Shutdown => {}
                }
            }
        }
        wait_for_shutdown(&mut stream);
    });

    tokio::task::spawn_blocking(move || connected_rx.recv_timeout(TEST_TIMEOUT))
        .await
        .unwrap()
        .unwrap();
    for sequence in 0..PACKET_COUNT {
        assert!(to_hook.try_send(packet(1, sequence, &[0x5a; PAYLOAD_BYTES])));
    }
    let received = tokio::task::spawn_blocking(move || {
        let deadline = Instant::now() + TEST_TIMEOUT;
        let mut received = 0;
        while received < PACKET_COUNT {
            let remaining = deadline.saturating_duration_since(Instant::now());
            received = progress_rx.recv_timeout(remaining).map_err(|_| received)?;
        }
        Ok::<_, u32>(received)
    })
    .await
    .unwrap();
    assert_eq!(received, Ok(PACKET_COUNT));

    while let Ok(event) = event_rx.try_recv() {
        if let RuntimeEvent::HookIpc(state) = event {
            assert_eq!(state.reconnects, 0, "local IPC unexpectedly reconnected");
        }
    }
    cancellation.cancel();
    time::timeout(TEST_TIMEOUT, worker)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    fake.join().unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires TRACTOR_BEAM_I686_IPC_PEER built for i686-pc-windows-msvc"]
async fn i686_peer_roundtrips_with_x64_client() {
    let peer_path = std::env::var_os("TRACTOR_BEAM_I686_IPC_PEER")
        .expect("set TRACTOR_BEAM_I686_IPC_PEER to the i686 ipc_test_peer executable");
    let session = HookIpcSession::test();
    let (control_tx, control_rx) = control_channel();
    let (event_tx, _event_rx) = tokio::sync::mpsc::channel(64);
    let cancellation = CancellationToken::new();
    let (mut from_hook, to_hook, worker) =
        start(session.clone(), control_rx, event_tx, cancellation.clone()).unwrap();
    let worker = tokio::spawn(worker);
    let mut peer = std::process::Command::new(peer_path)
        .arg(&session.endpoint)
        .arg(session.session_id.to_hex())
        .spawn()
        .unwrap();

    let received = time::timeout(TEST_TIMEOUT, from_hook.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        received,
        GamePacket {
            peer: 42,
            sequence: 8,
            channel: 3,
            send_type: 2,
            payload: b"i686-hook-to-x64-client".to_vec(),
        }
    );
    assert!(to_hook.try_send(GamePacket {
        peer: 41,
        sequence: 7,
        channel: 3,
        send_type: 2,
        payload: b"x64-client-to-i686-hook".to_vec(),
    }));
    let result = tokio::task::spawn_blocking(move || {
        request_input_delay(&control_tx, 91, InputDelayCommand::Read)
    })
    .await
    .unwrap()
    .unwrap();
    assert_eq!(result, Ok(37));

    cancellation.cancel();
    time::timeout(TEST_TIMEOUT, worker)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let status = tokio::task::spawn_blocking(move || peer.wait())
        .await
        .unwrap()
        .unwrap();
    assert!(status.success());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires packaged i686 Hook, i686 hook_loader, and Isaac steam_api.dll paths"]
async fn packaged_i686_hook_handshakes_with_x64_client() {
    let loader = std::env::var_os("TRACTOR_BEAM_I686_HOOK_LOADER")
        .expect("set TRACTOR_BEAM_I686_HOOK_LOADER");
    let steam_api = std::env::var_os("TRACTOR_BEAM_STEAM_API").expect("set TRACTOR_BEAM_STEAM_API");
    let packaged_hook =
        std::env::var_os("TRACTOR_BEAM_PACKAGED_HOOK").expect("set TRACTOR_BEAM_PACKAGED_HOOK");
    let directory = tempfile::tempdir().unwrap();
    let temp = directory.path();
    let hook = temp.join("tractor_beam_native_hook.dll");
    std::fs::copy(packaged_hook, &hook).unwrap();

    let session = HookIpcSession::test();
    let hook_logs = temp.join("logs").join("hook");
    std::fs::create_dir_all(&hook_logs).unwrap();
    std::fs::write(
        hook_logs.join("hook-runtime.txt"),
        format!(
            "mode=replace\nfallback_to_steam=1\nipc_endpoint={}\nipc_session={}\n",
            session.endpoint,
            session.session_id.to_hex()
        ),
    )
    .unwrap();
    let (control_tx, control_rx) = control_channel();
    let (event_tx, mut event_rx) = tokio::sync::mpsc::channel(64);
    let cancellation = CancellationToken::new();
    let (_from_hook, _to_hook, worker) =
        start(session, control_rx, event_tx, cancellation.clone()).unwrap();
    let worker = tokio::spawn(worker);
    let mut loader = std::process::Command::new(loader)
        .arg(steam_api)
        .arg(&hook)
        .spawn()
        .unwrap();

    time::timeout(TEST_TIMEOUT, async {
        loop {
            if let Some(RuntimeEvent::HookIpc(state)) = event_rx.recv().await
                && state.connection == HookIpcConnectionState::Connected
            {
                return;
            }
        }
    })
    .await
    .unwrap();
    let input_delay = tokio::task::spawn_blocking(move || {
        request_input_delay(&control_tx, 101, InputDelayCommand::Read)
    })
    .await
    .unwrap()
    .unwrap();
    assert!(input_delay.is_err());

    cancellation.cancel();
    time::timeout(TEST_TIMEOUT, worker)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let status = tokio::task::spawn_blocking(move || loader.wait())
        .await
        .unwrap()
        .unwrap();
    assert!(status.success());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn wrong_session_is_terminal_without_fallback() {
    let session = HookIpcSession::test();
    let (_control_tx, control_rx) = control_channel();
    let (event_tx, _event_rx) = tokio::sync::mpsc::channel(16);
    let cancellation = CancellationToken::new();
    let (_from_hook, _to_hook, worker) =
        start(session.clone(), control_rx, event_tx, cancellation).unwrap();
    let worker = tokio::spawn(worker);
    let fake = thread::spawn(move || {
        let name = session.endpoint.to_ns_name::<GenericNamespaced>().unwrap();
        let mut stream = connect_with_retry(name);
        let wrong = SessionId::new([0xff; 16]);
        write_hook_message(
            &mut stream,
            &HookToClient::Handshake(Handshake::new(PeerRole::NativeHook, wrong)),
        )
        .unwrap();
    });

    let error = time::timeout(TEST_TIMEOUT, worker)
        .await
        .unwrap()
        .unwrap()
        .unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    assert!(error.to_string().contains("session identity mismatch"));
    fake.join().unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn malformed_frame_after_handshake_is_terminal() {
    let session = HookIpcSession::test();
    let (_control_tx, control_rx) = control_channel();
    let (event_tx, _event_rx) = tokio::sync::mpsc::channel(16);
    let cancellation = CancellationToken::new();
    let (_from_hook, _to_hook, worker) =
        start(session.clone(), control_rx, event_tx, cancellation).unwrap();
    let worker = tokio::spawn(worker);
    let fake = thread::spawn(move || {
        let mut stream = connect_fake_hook(&session);
        stream.write_all(&[0xff, 0]).unwrap();
    });

    let error = time::timeout(TEST_TIMEOUT, worker)
        .await
        .unwrap()
        .unwrap()
        .unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    fake.join().unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn absent_hook_times_out_with_test_budget() {
    let session = HookIpcSession::test();
    let (_control_tx, control_rx) = control_channel();
    let (event_tx, _event_rx) = tokio::sync::mpsc::channel(16);
    let cancellation = CancellationToken::new();
    let (_from_hook, _to_hook, worker) = start_with_settings(
        session,
        control_rx,
        event_tx,
        cancellation,
        ListenerSettings {
            accept_poll_interval: Duration::from_millis(2),
            initial_connect_timeout: Duration::from_millis(30),
            reconnect_timeout: Duration::from_millis(30),
        },
    )
    .unwrap();

    let error = time::timeout(TEST_TIMEOUT, worker)
        .await
        .unwrap()
        .unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::TimedOut);
    assert!(error.to_string().contains("connection timed out"));
}

#[test]
fn full_client_queue_drops_newest_without_blocking() {
    let session = HookIpcSession::test();
    let (_control_tx, control_rx) = control_channel();
    let (event_tx, _event_rx) = tokio::sync::mpsc::channel(16);
    let (_from_hook, to_hook, _worker) =
        start(session, control_rx, event_tx, CancellationToken::new()).unwrap();
    for sequence in 0..tractor_beam_hook_ipc::CLIENT_DATA_QUEUE_CAPACITY {
        assert!(to_hook.try_send(packet(1, sequence as u32, b"queued")));
    }

    assert!(!to_hook.try_send(packet(1, u32::MAX, b"dropped")));
    assert_eq!(to_hook.dropped.load(Ordering::Relaxed), 1);
}
