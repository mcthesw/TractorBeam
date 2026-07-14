use std::io;

use tokio::{
    io::{AsyncReadExt as _, AsyncWriteExt as _},
    net::TcpStream,
};

use crate::protocol::{
    BOOTSTRAP_SCHEMA, BootstrapMessage, BuildMetadata, CAP_RESUME, CAP_ROOM_PATH_PROBE,
    CAP_TCP_DATA, CAP_UDP_DATA, ProtocolRange, decode_bootstrap, encode_bootstrap,
};

use super::TransportChoice;

pub(super) async fn negotiate(
    stream: &mut TcpStream,
    choice: TransportChoice,
    client_version: &str,
    git_hash: Option<&str>,
) -> io::Result<u64> {
    let profile_capability = match choice {
        TransportChoice::Tcp => CAP_TCP_DATA,
        TransportChoice::Udp => CAP_UDP_DATA,
    };
    let hello = BootstrapMessage::ClientHello {
        bootstrap_schema: BOOTSTRAP_SCHEMA,
        supported_protocol_ranges: vec![ProtocolRange {
            major: 2,
            min_minor: 0,
            max_minor: 0,
        }],
        required_capabilities: CAP_RESUME | profile_capability,
        optional_capabilities: CAP_TCP_DATA | CAP_UDP_DATA | CAP_ROOM_PATH_PROBE,
        client: BuildMetadata {
            version: client_version.to_owned(),
            git_hash: git_hash.map(str::to_owned),
        },
    };
    stream
        .write_all(&encode_bootstrap(&hello).map_err(io::Error::other)?)
        .await?;
    let response = read_bootstrap(stream).await?;
    match decode_bootstrap(&response).map_err(io::Error::other)? {
        BootstrapMessage::ServerHello {
            selected_protocol,
            enabled_capabilities,
            ..
        } if selected_protocol.major == 2
            && enabled_capabilities & (CAP_RESUME | profile_capability)
                == CAP_RESUME | profile_capability =>
        {
            Ok(enabled_capabilities)
        }
        BootstrapMessage::CompatibilityReject(reject) => Err(io::Error::new(
            io::ErrorKind::Unsupported,
            format!("Relay compatibility rejected: {:?}", reject.code),
        )),
        _ => Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "Relay selected an incompatible data profile",
        )),
    }
}

async fn read_bootstrap(stream: &mut TcpStream) -> io::Result<Vec<u8>> {
    let length = usize::try_from(stream.read_u32().await?).map_err(io::Error::other)?;
    if length > crate::protocol::MAX_BOOTSTRAP_PAYLOAD {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Relay bootstrap is too large",
        ));
    }
    let mut payload = vec![0_u8; length];
    stream.read_exact(&mut payload).await?;
    let mut frame = Vec::with_capacity(4 + length);
    frame.extend_from_slice(
        &u32::try_from(length)
            .map_err(io::Error::other)?
            .to_be_bytes(),
    );
    frame.extend_from_slice(&payload);
    Ok(frame)
}
