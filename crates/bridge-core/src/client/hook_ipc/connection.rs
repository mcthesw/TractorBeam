use super::*;

pub(super) fn server_handshake(
    stream: &mut LocalSocketStream,
    session_id: SessionId,
) -> io::Result<(
    tractor_beam_hook_ipc::NegotiatedProtocol,
    FrameDecoder,
    Vec<HookToClient>,
)> {
    stream.set_nonblocking(true)?;
    let deadline = Instant::now() + HANDSHAKE_TIMEOUT;
    let mut decoder = FrameDecoder::new();
    let negotiated = 'handshake: loop {
        if Instant::now() >= deadline {
            return Err(protocol_io("local IPC handshake timed out"));
        }
        match read_messages::<HookToClient>(stream, &mut decoder) {
            Ok(messages) => match messages.as_slice() {
                [HookToClient::Handshake(handshake)] => {
                    break 'handshake (*handshake)
                        .validate(PeerRole::NativeHook, session_id)
                        .map_err(protocol_io)?;
                }
                [] => {}
                _ => return Err(protocol_io("expected one Native Hook handshake")),
            },
            Err(error) if is_transient(&error) => thread::sleep(IO_POLL_INTERVAL),
            Err(error) => return Err(error),
        }
    };
    write_message(
        stream,
        &ClientToHook::Handshake(Handshake::new(PeerRole::BridgeClient, session_id)),
    )?;
    loop {
        if Instant::now() >= deadline {
            return Err(protocol_io("local IPC ready acknowledgement timed out"));
        }
        match read_messages::<HookToClient>(stream, &mut decoder) {
            Ok(messages) => {
                let mut messages = messages.into_iter();
                if let Some(message) = messages.next() {
                    if message == HookToClient::Ready {
                        return Ok((negotiated, decoder, messages.collect()));
                    }
                    return Err(protocol_io("expected Native Hook ready acknowledgement"));
                }
            }
            Err(error) if is_transient(&error) => thread::sleep(IO_POLL_INTERVAL),
            Err(error) => return Err(error),
        }
    }
}

pub(super) enum ConnectionEnd {
    Shutdown,
    Disconnected,
}

pub(super) fn run_connection(
    stream: &mut LocalSocketStream,
    context: &ConnectionContext<'_>,
    reconnects: u32,
    mut decoder: FrameDecoder,
    mut pending_messages: Vec<HookToClient>,
) -> io::Result<ConnectionEnd> {
    let mut pending = HashMap::<u32, SyncSender<Result<i32, ErrorCode>>>::new();
    let mut next_ping_at = Instant::now() + LIVENESS_PING_INTERVAL;
    let mut pending_ping = None::<(u32, Instant)>;
    let mut next_ping_id = 1_u32;
    loop {
        if context.cancellation.is_cancelled() {
            let _ = write_message(stream, &ClientToHook::Shutdown);
            reject_pending(&mut pending);
            return Ok(ConnectionEnd::Shutdown);
        }

        while let Ok(call) = context.control_rx.try_recv() {
            write_message(
                stream,
                &ClientToHook::InputDelay {
                    id: call.id,
                    command: call.command,
                },
            )?;
            pending.insert(call.id, call.response);
        }

        let now = Instant::now();
        if pending_ping.is_some_and(|(_, deadline)| now >= deadline) {
            reject_pending(&mut pending);
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "Native Hook local IPC liveness check timed out",
            ));
        }
        if pending_ping.is_none() && now >= next_ping_at {
            let id = next_ping_id;
            next_ping_id = next_ping_id.wrapping_add(1);
            write_message(stream, &ClientToHook::Ping { id })?;
            pending_ping = Some((id, now + LIVENESS_PONG_TIMEOUT));
            next_ping_at = now + LIVENESS_PING_INTERVAL;
        }

        let messages = if pending_messages.is_empty() {
            read_messages::<HookToClient>(stream, &mut decoder)
        } else {
            Ok(mem::take(&mut pending_messages))
        };
        match messages {
            Ok(messages) => {
                for message in messages {
                    match message {
                        HookToClient::Handshake(_) | HookToClient::Ready => {
                            reject_pending(&mut pending);
                            return Err(protocol_io("unexpected handshake message after ready"));
                        }
                        HookToClient::Game(packet) => {
                            if context.from_hook_tx.try_send(packet).is_err() {
                                saturating_increment(context.client_dropped);
                            }
                        }
                        HookToClient::InputDelayResult { id, result } => {
                            if let Some(response) = pending.remove(&id) {
                                let _ = response.send(result);
                            } else {
                                reject_pending(&mut pending);
                                return Err(protocol_io("unexpected Input Delay response id"));
                            }
                        }
                        HookToClient::Pong { id } => match pending_ping {
                            Some((expected, _)) if expected == id => pending_ping = None,
                            _ => {
                                reject_pending(&mut pending);
                                return Err(protocol_io("unexpected local IPC liveness response"));
                            }
                        },
                        HookToClient::Health(health) => publish_status(
                            context.event_tx,
                            HookIpcState {
                                connection: HookIpcConnectionState::Connected,
                                negotiated_major: Some(tractor_beam_hook_ipc::PROTOCOL_MAJOR),
                                negotiated_minor: Some(tractor_beam_hook_ipc::PROTOCOL_MINOR),
                                reconnects: reconnects.max(health.reconnects),
                                hook_data_dropped: health.hook_data_dropped,
                                client_data_dropped: context
                                    .client_dropped
                                    .load(Ordering::Relaxed)
                                    .max(health.client_data_dropped),
                                malformed_frames: health.malformed_frames,
                                updated_at: unix_seconds(),
                                ..HookIpcState::default()
                            },
                        ),
                        HookToClient::Goodbye => {
                            reject_pending(&mut pending);
                            return Ok(ConnectionEnd::Disconnected);
                        }
                    }
                }
            }
            Err(error) if is_transient(&error) => {}
            Err(error) if is_disconnect(&error) => {
                reject_pending(&mut pending);
                return Ok(ConnectionEnd::Disconnected);
            }
            Err(error) => {
                reject_pending(&mut pending);
                return Err(error);
            }
        }

        for _ in 0..MAX_DATA_BURST {
            match context.to_hook_rx.try_recv() {
                Ok(packet) => write_message(stream, &ClientToHook::Game(packet))?,
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    reject_pending(&mut pending);
                    return Ok(ConnectionEnd::Shutdown);
                }
            }
        }
    }
}

pub(super) fn reject_pending(pending: &mut HashMap<u32, SyncSender<Result<i32, ErrorCode>>>) {
    for (_, response) in pending.drain() {
        let _ = response.send(Err(ErrorCode::NotConnected));
    }
}

pub(super) fn reject_pending_controls(control_rx: &Receiver<InputDelayCall>) {
    while let Ok(call) = control_rx.try_recv() {
        let _ = call.response.send(Err(ErrorCode::NotConnected));
    }
}

pub(super) fn drain_data(data_rx: &Receiver<GamePacket>, dropped: &AtomicU64) {
    while data_rx.try_recv().is_ok() {
        saturating_increment(dropped);
    }
}

pub(super) fn publish_failure(event_tx: &RuntimeEventSender, message: &str) {
    publish_status(
        event_tx,
        HookIpcState {
            connection: HookIpcConnectionState::Failed,
            last_error: Some(message.to_owned()),
            updated_at: unix_seconds(),
            ..HookIpcState::default()
        },
    );
    send_event(
        event_tx,
        log_event(
            LogLevel::Error,
            format!("Native Hook local IPC failed: {message}"),
        ),
    );
}

pub(super) fn publish_status(event_tx: &RuntimeEventSender, state: HookIpcState) {
    send_event(event_tx, RuntimeEvent::HookIpc(Box::new(state)));
}

pub(super) fn status(connection: HookIpcConnectionState) -> HookIpcState {
    HookIpcState {
        connection,
        updated_at: unix_seconds(),
        ..HookIpcState::default()
    }
}

pub(super) fn write_message(
    stream: &mut LocalSocketStream,
    message: &ClientToHook,
) -> io::Result<()> {
    let encoded = tractor_beam_hook_ipc::encode(message).map_err(protocol_io)?;
    write_all_bounded(stream, &encoded)
}

pub(super) fn write_all_bounded(stream: &mut LocalSocketStream, bytes: &[u8]) -> io::Result<()> {
    let deadline = Instant::now() + WRITE_TIMEOUT;
    let mut written = 0;
    while written < bytes.len() {
        match stream.write(&bytes[written..]) {
            Ok(0) => {
                return Err(io::Error::new(
                    io::ErrorKind::WriteZero,
                    "local IPC stream stopped accepting bytes",
                ));
            }
            Ok(size) => written += size,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) if is_transient(&error) && Instant::now() < deadline => {
                thread::sleep(IO_POLL_INTERVAL);
            }
            Err(error) if is_transient(&error) => {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "local IPC write timed out",
                ));
            }
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

pub(super) fn read_messages<T: tractor_beam_hook_ipc::WireMessage>(
    stream: &mut LocalSocketStream,
    decoder: &mut FrameDecoder,
) -> io::Result<Vec<T>> {
    let mut buffer = [0_u8; 4_096];
    match stream.read(&mut buffer) {
        #[cfg(windows)]
        Ok(0) => Err(io::Error::new(
            io::ErrorKind::WouldBlock,
            "local IPC named pipe has no bytes available",
        )),
        #[cfg(not(windows))]
        Ok(0) => Err(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            "local IPC peer disconnected",
        )),
        Ok(size) => decoder.push(&buffer[..size]).map_err(protocol_io),
        Err(error) => Err(error),
    }
}

pub(super) fn protocol_io(error: impl ToString) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, error.to_string())
}

pub(super) fn is_protocol_error(error: &io::Error) -> bool {
    error.kind() == io::ErrorKind::InvalidData
}

pub(super) fn is_transient(error: &io::Error) -> bool {
    matches!(
        error.kind(),
        io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
    )
}

pub(super) fn is_disconnect(error: &io::Error) -> bool {
    matches!(
        error.kind(),
        io::ErrorKind::UnexpectedEof
            | io::ErrorKind::BrokenPipe
            | io::ErrorKind::ConnectionAborted
            | io::ErrorKind::ConnectionReset
    )
}

pub(super) fn saturating_increment(counter: &AtomicU64) {
    let _ = counter.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |value| {
        Some(value.saturating_add(1))
    });
}
