use std::{
    io,
    net::{SocketAddr, TcpStream},
    thread,
    time::{Duration, Instant},
};

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
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "missing endpoint"))?
        .parse::<SocketAddr>()
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))?;
    if !endpoint.is_ipv4() || !endpoint.ip().is_loopback() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "endpoint must be an IPv4 loopback address",
        ));
    }
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

    let mut stream = connect(endpoint)?;
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

fn connect(endpoint: SocketAddr) -> io::Result<TcpStream> {
    let deadline = Instant::now() + TEST_TIMEOUT;
    loop {
        match TcpStream::connect(endpoint) {
            Ok(stream) => {
                stream.set_nodelay(true)?;
                stream.set_nonblocking(true)?;
                return Ok(stream);
            }
            Err(_) if Instant::now() < deadline => thread::sleep(POLL_INTERVAL),
            Err(error) => return Err(error),
        }
    }
}

fn handshake(stream: &mut TcpStream, session_id: SessionId) -> io::Result<()> {
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
                write_message(stream, &HookToClient::EndpointReady)?;
                write_message(
                    stream,
                    &HookToClient::Startup(tractor_beam_hook_ipc::HookStartupStatus::Ready {
                        steam_id64: Some(76561198000000000),
                    }),
                )?;
                return Ok(());
            }
            [] => {}
            _ => return Err(protocol_io("expected one Bridge Client handshake")),
        }
    }
}

fn write_message(stream: &mut TcpStream, message: &HookToClient) -> io::Result<()> {
    tractor_beam_hook_ipc::sync_io::write_message(stream, message, WRITE_TIMEOUT, POLL_INTERVAL)
}

fn read_messages<T: WireMessage>(
    stream: &mut TcpStream,
    decoder: &mut FrameDecoder,
) -> io::Result<Vec<T>> {
    match tractor_beam_hook_ipc::sync_io::read_messages(stream, decoder) {
        Err(error) if is_transient(&error) => {
            thread::sleep(POLL_INTERVAL);
            Ok(Vec::new())
        }
        result => result,
    }
}

fn protocol_io(error: impl ToString) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, error.to_string())
}

fn is_transient(error: &io::Error) -> bool {
    tractor_beam_hook_ipc::sync_io::is_transient(error)
}
