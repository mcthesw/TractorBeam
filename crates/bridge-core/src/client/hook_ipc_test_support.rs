use super::*;

pub(super) const TEST_TIMEOUT: Duration = Duration::from_secs(3);

pub(super) fn connect_fake_hook(session: &HookIpcSession) -> LocalSocketStream {
    let name = session
        .endpoint
        .clone()
        .to_ns_name::<GenericNamespaced>()
        .unwrap();
    let mut stream = connect_with_retry(name);
    stream.set_nonblocking(true).unwrap();
    write_hook_message(
        &mut stream,
        &HookToClient::Handshake(Handshake::new(PeerRole::NativeHook, session.session_id)),
    )
    .unwrap();
    let mut decoder = FrameDecoder::new();
    let deadline = Instant::now() + TEST_TIMEOUT;
    loop {
        assert!(Instant::now() < deadline, "client handshake timed out");
        if let Some(message) = read_client_messages(&mut stream, &mut decoder)
            .unwrap()
            .into_iter()
            .next()
        {
            match message {
                ClientToHook::Handshake(handshake) => {
                    handshake
                        .validate(PeerRole::BridgeClient, session.session_id)
                        .unwrap();
                    write_hook_message(&mut stream, &HookToClient::Ready).unwrap();
                    return stream;
                }
                _ => panic!("expected client handshake"),
            }
        }
    }
}

pub(super) fn connect_with_retry(
    name: interprocess::local_socket::Name<'static>,
) -> LocalSocketStream {
    let deadline = Instant::now() + TEST_TIMEOUT;
    loop {
        match LocalSocketStream::connect(name.clone()) {
            Ok(stream) => return stream,
            Err(error) if Instant::now() < deadline => {
                let _ = error;
                thread::sleep(Duration::from_millis(5));
            }
            Err(error) => panic!("fake Hook failed to connect: {error}"),
        }
    }
}

pub(super) fn read_client_messages(
    stream: &mut LocalSocketStream,
    decoder: &mut FrameDecoder,
) -> io::Result<Vec<ClientToHook>> {
    match read_messages(stream, decoder) {
        Err(error) if is_transient(&error) => Ok(Vec::new()),
        result => result,
    }
}

pub(super) fn write_hook_message(
    stream: &mut LocalSocketStream,
    message: &HookToClient,
) -> io::Result<()> {
    tractor_beam_hook_ipc::sync_io::write_message(stream, message, WRITE_TIMEOUT, IO_POLL_INTERVAL)
}

pub(super) fn wait_for_shutdown(stream: &mut LocalSocketStream) {
    let mut decoder = FrameDecoder::new();
    let deadline = Instant::now() + TEST_TIMEOUT;
    loop {
        assert!(Instant::now() < deadline, "client shutdown timed out");
        for message in read_client_messages(stream, &mut decoder).unwrap() {
            match message {
                ClientToHook::Ping { id } => {
                    write_hook_message(stream, &HookToClient::Pong { id }).unwrap();
                }
                ClientToHook::Shutdown => return,
                _ => {}
            }
        }
    }
}

pub(super) fn packet(peer: u64, sequence: u32, payload: &[u8]) -> GamePacket {
    GamePacket {
        peer,
        sequence,
        channel: 3,
        send_type: 2,
        payload: payload.to_vec(),
    }
}
