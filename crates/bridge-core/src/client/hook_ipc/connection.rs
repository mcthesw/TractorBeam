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
    decoder: FrameDecoder,
    mut pending_messages: Vec<HookToClient>,
) -> io::Result<ConnectionEnd> {
    let mut read_stream = stream.try_clone()?;
    read_stream.set_nonblocking(false)?;
    let (reader_tx, reader_rx) = mpsc::channel::<io::Result<HookToClient>>();
    let from_hook_tx = context.from_hook_tx.clone();
    let client_dropped = Arc::clone(context.client_dropped);
    thread::spawn(move || {
        let mut decoder = decoder;
        loop {
            match read_messages::<HookToClient>(&mut read_stream, &mut decoder) {
                Ok(messages) => {
                    for message in messages {
                        if let HookToClient::Game(packet) = message {
                            if from_hook_tx.try_send(packet).is_err() {
                                saturating_increment(&client_dropped);
                            }
                        } else {
                            let terminal = message == HookToClient::Goodbye;
                            if reader_tx.send(Ok(message)).is_err() || terminal {
                                return;
                            }
                        }
                    }
                }
                Err(error) if is_transient(&error) => {}
                Err(error) => {
                    let _ = reader_tx.send(Err(error));
                    return;
                }
            }
        }
    });

    let mut pending = HashMap::<u32, SyncSender<Result<i32, ErrorCode>>>::new();
    let mut pending_write = None::<PendingWrite>;
    let mut next_ping_at = Instant::now() + LIVENESS_PING_INTERVAL;
    let mut pending_ping = None::<(u32, Instant)>;
    let mut next_ping_id = 1_u32;
    loop {
        if context.cancellation.is_cancelled() {
            let _ = write_message(stream, &ClientToHook::Shutdown);
            reject_pending(&mut pending);
            return Ok(ConnectionEnd::Shutdown);
        }

        let now = Instant::now();
        if pending_ping.is_some_and(|(_, deadline)| now >= deadline) {
            reject_pending(&mut pending);
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "Native Hook local IPC liveness check timed out",
            ));
        }
        let messages: Vec<io::Result<HookToClient>> = if pending_messages.is_empty() {
            reader_rx.try_iter().collect()
        } else {
            mem::take(&mut pending_messages)
                .into_iter()
                .map(Ok)
                .collect()
        };
        for message in messages {
            match message {
                Ok(message) => match message {
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
                },
                Err(error) if is_disconnect(&error) => {
                    reject_pending(&mut pending);
                    return Ok(ConnectionEnd::Disconnected);
                }
                Err(error) => {
                    reject_pending(&mut pending);
                    return Err(error);
                }
            }
        }

        if let Some(write) = &mut pending_write {
            if write.try_flush(stream)? {
                pending_write = None;
            } else {
                thread::sleep(IO_POLL_INTERVAL);
                continue;
            }
        }

        while let Ok(call) = context.control_rx.try_recv() {
            let message = ClientToHook::InputDelay {
                id: call.id,
                command: call.command,
            };
            pending.insert(call.id, call.response);
            pending_write = PendingWrite::start(stream, &message)?;
            if pending_write.is_some() {
                break;
            }
        }

        let now = Instant::now();
        if pending_write.is_none() && pending_ping.is_none() && now >= next_ping_at {
            let id = next_ping_id;
            next_ping_id = next_ping_id.wrapping_add(1);
            pending_write = PendingWrite::start(stream, &ClientToHook::Ping { id })?;
            pending_ping = Some((id, now + LIVENESS_PONG_TIMEOUT));
            next_ping_at = now + LIVENESS_PING_INTERVAL;
        }

        if pending_write.is_some() {
            continue;
        }

        for _ in 0..MAX_DATA_BURST {
            match context.to_hook_rx.try_recv() {
                Ok(packet) => {
                    pending_write = PendingWrite::start(stream, &ClientToHook::Game(packet))?;
                    if pending_write.is_some() {
                        break;
                    }
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    reject_pending(&mut pending);
                    return Ok(ConnectionEnd::Shutdown);
                }
            }
        }
    }
}

struct PendingWrite {
    bytes: Vec<u8>,
    written: usize,
    stalled_since: Instant,
}

impl PendingWrite {
    fn start(stream: &mut impl Write, message: &ClientToHook) -> io::Result<Option<PendingWrite>> {
        let bytes = tractor_beam_hook_ipc::encode(message).map_err(protocol_io)?;
        let mut write = PendingWrite {
            bytes,
            written: 0,
            stalled_since: Instant::now(),
        };
        if write.try_flush(stream)? {
            Ok(None)
        } else {
            Ok(Some(write))
        }
    }

    fn try_flush(&mut self, stream: &mut impl Write) -> io::Result<bool> {
        match stream.write(&self.bytes[self.written..]) {
            Ok(0) if self.stalled_since.elapsed() < WRITE_TIMEOUT => Ok(false),
            Ok(0) => Err(write_timeout()),
            Ok(size) => {
                self.written = self.written.saturating_add(size);
                self.stalled_since = Instant::now();
                Ok(self.written >= self.bytes.len())
            }
            Err(error) if error.kind() == io::ErrorKind::Interrupted => Ok(false),
            Err(error) if is_transient(&error) && self.stalled_since.elapsed() < WRITE_TIMEOUT => {
                Ok(false)
            }
            Err(error) if is_transient(&error) => Err(write_timeout()),
            Err(error) => Err(error),
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

pub(super) fn write_all_bounded(stream: &mut impl Write, bytes: &[u8]) -> io::Result<()> {
    let deadline = Instant::now() + WRITE_TIMEOUT;
    let mut written = 0;
    while written < bytes.len() {
        match stream.write(&bytes[written..]) {
            #[cfg(windows)]
            Ok(0) if Instant::now() < deadline => thread::sleep(IO_POLL_INTERVAL),
            #[cfg(windows)]
            Ok(0) => return Err(write_timeout()),
            #[cfg(not(windows))]
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
                return Err(write_timeout());
            }
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

fn write_timeout() -> io::Error {
    io::Error::new(io::ErrorKind::TimedOut, "local IPC write timed out")
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

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    struct ZeroThenWrite {
        returned_zero: bool,
        bytes: Vec<u8>,
    }

    impl Write for ZeroThenWrite {
        fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
            if !self.returned_zero {
                self.returned_zero = true;
                return Ok(0);
            }
            self.bytes.extend_from_slice(buffer);
            Ok(buffer.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn retries_zero_progress_windows_named_pipe_write() {
        let mut writer = ZeroThenWrite {
            returned_zero: false,
            bytes: Vec::new(),
        };

        write_all_bounded(&mut writer, b"game-packet").unwrap();

        assert!(writer.returned_zero);
        assert_eq!(writer.bytes, b"game-packet");
    }

    #[test]
    fn pending_write_resumes_after_zero_progress() {
        let mut writer = ZeroThenWrite {
            returned_zero: false,
            bytes: Vec::new(),
        };
        let mut pending = PendingWrite {
            bytes: b"game-packet".to_vec(),
            written: 0,
            stalled_since: Instant::now(),
        };

        assert!(!pending.try_flush(&mut writer).unwrap());
        assert!(pending.try_flush(&mut writer).unwrap());

        assert_eq!(writer.bytes, b"game-packet");
    }
}
