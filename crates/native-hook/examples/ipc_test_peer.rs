use std::{
    io::{self, Read, Write},
    thread,
    time::{Duration, Instant},
};

use interprocess::local_socket::{GenericNamespaced, prelude::*};
use tractor_beam_hook_ipc::{
    ClientToHook, FrameDecoder, GamePacket, Handshake, HookToClient, IpcHealth, PeerRole,
    SessionId, WireMessage,
};

const TEST_TIMEOUT: Duration = Duration::from_secs(5);
const POLL_INTERVAL: Duration = Duration::from_millis(5);
const WRITE_TIMEOUT: Duration = Duration::from_millis(250);

fn main() -> io::Result<()> {
    let mut arguments = std::env::args().skip(1);
    let endpoint = arguments
        .next()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "missing endpoint"))?;
    let session_id = arguments
        .next()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "missing session identity"))?
        .parse::<SessionId>()
        .map_err(protocol_io)?;
    if arguments.next().is_some() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "unexpected argument",
        ));
    }

    let mut stream = connect(&endpoint)?;
    handshake(&mut stream, session_id)?;
    write_message(&mut stream, &HookToClient::Health(IpcHealth::default()))?;
    write_message(
        &mut stream,
        &HookToClient::Game(GamePacket {
            peer: 42,
            sequence: 8,
            channel: 3,
            send_type: 2,
            payload: b"i686-hook-to-x64-client".to_vec(),
        }),
    )?;

    let deadline = Instant::now() + TEST_TIMEOUT;
    let mut decoder = FrameDecoder::new();
    let mut saw_game = false;
    let mut saw_input_delay = false;
    loop {
        if Instant::now() >= deadline {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "cross-architecture IPC test timed out",
            ));
        }
        for message in read_messages::<ClientToHook>(&mut stream, &mut decoder)? {
            match message {
                ClientToHook::Handshake(_) => {
                    return Err(protocol_io("duplicate Bridge Client handshake"));
                }
                ClientToHook::Game(packet) => {
                    if packet.peer != 41
                        || packet.sequence != 7
                        || packet.payload != b"x64-client-to-i686-hook"
                    {
                        return Err(protocol_io("unexpected Client game packet"));
                    }
                    saw_game = true;
                }
                ClientToHook::InputDelay { id, .. } => {
                    write_message(
                        &mut stream,
                        &HookToClient::InputDelayResult { id, result: Ok(37) },
                    )?;
                    saw_input_delay = true;
                }
                ClientToHook::Ping { id } => {
                    write_message(&mut stream, &HookToClient::Pong { id })?;
                }
                ClientToHook::Shutdown => {
                    if saw_game && saw_input_delay {
                        return Ok(());
                    }
                    return Err(protocol_io(
                        "Client shut down before cross-architecture traffic completed",
                    ));
                }
            }
        }
    }
}

fn connect(endpoint: &str) -> io::Result<LocalSocketStream> {
    let name = endpoint
        .to_owned()
        .to_ns_name::<GenericNamespaced>()
        .map_err(io::Error::other)?;
    let deadline = Instant::now() + TEST_TIMEOUT;
    loop {
        match LocalSocketStream::connect(name.clone()) {
            Ok(stream) => {
                stream.set_nonblocking(true)?;
                return Ok(stream);
            }
            Err(_) if Instant::now() < deadline => thread::sleep(POLL_INTERVAL),
            Err(error) => return Err(error),
        }
    }
}

fn handshake(stream: &mut LocalSocketStream, session_id: SessionId) -> io::Result<()> {
    write_message(
        stream,
        &HookToClient::Handshake(Handshake::new(PeerRole::NativeHook, session_id)),
    )?;
    let deadline = Instant::now() + TEST_TIMEOUT;
    let mut decoder = FrameDecoder::new();
    loop {
        if Instant::now() >= deadline {
            return Err(protocol_io("Bridge Client handshake timed out"));
        }
        let messages = read_messages::<ClientToHook>(stream, &mut decoder)?;
        match messages.as_slice() {
            [ClientToHook::Handshake(handshake)] => {
                (*handshake)
                    .validate(PeerRole::BridgeClient, session_id)
                    .map_err(protocol_io)?;
                write_message(stream, &HookToClient::Ready)?;
                return Ok(());
            }
            [] => {}
            _ => return Err(protocol_io("expected one Bridge Client handshake")),
        }
    }
}

fn write_message(stream: &mut LocalSocketStream, message: &HookToClient) -> io::Result<()> {
    let encoded = tractor_beam_hook_ipc::encode(message).map_err(protocol_io)?;
    let deadline = Instant::now() + WRITE_TIMEOUT;
    let mut written = 0;
    while written < encoded.len() {
        match stream.write(&encoded[written..]) {
            Ok(0) => {
                return Err(io::Error::new(
                    io::ErrorKind::WriteZero,
                    "local IPC stream stopped accepting bytes",
                ));
            }
            Ok(size) => written += size,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) if is_transient(&error) && Instant::now() < deadline => {
                thread::sleep(POLL_INTERVAL);
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

fn read_messages<T: WireMessage>(
    stream: &mut LocalSocketStream,
    decoder: &mut FrameDecoder,
) -> io::Result<Vec<T>> {
    let mut buffer = [0_u8; 4_096];
    match stream.read(&mut buffer) {
        Ok(0) => {
            thread::sleep(POLL_INTERVAL);
            Ok(Vec::new())
        }
        Ok(size) => decoder.push(&buffer[..size]).map_err(protocol_io),
        Err(error) if is_transient(&error) => {
            thread::sleep(POLL_INTERVAL);
            Ok(Vec::new())
        }
        Err(error) => Err(error),
    }
}

fn protocol_io(error: impl ToString) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, error.to_string())
}

fn is_transient(error: &io::Error) -> bool {
    matches!(
        error.kind(),
        io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
    )
}
