//! EOS P2P payloads carried by `GamePacket` between compatible Native Hooks.

use super::GamePacket;

pub const MAX_PACKET: usize = 1170;
const MAX_SOCKET_NAME: usize = 32;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Reliability {
    UnreliableUnordered,
    ReliableUnordered,
    ReliableOrdered,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Packet<'a> {
    pub socket: &'a str,
    pub channel: u8,
    pub reliability: Reliability,
    pub data: &'a [u8],
}

impl Packet<'_> {
    /// The caller must resolve the EOS remote user to a room member's Steam ID.
    /// A PUID, its address, or a truncated hash is not a Relay destination.
    pub fn encode(&self, peer_steam_id: u64, sequence: u32) -> Option<GamePacket> {
        if peer_steam_id == 0 || !valid(self.socket, self.data) {
            return None;
        }
        let mut payload = Vec::with_capacity(1 + self.socket.len() + self.data.len());
        payload.push(u8::try_from(self.socket.len()).ok()?);
        payload.extend_from_slice(self.socket.as_bytes());
        payload.extend_from_slice(self.data);
        Some(GamePacket {
            peer: peer_steam_id,
            sequence,
            channel: self.channel.into(),
            // Steam's send modes are non-negative. Do not interpret Steam
            // payload bytes as an EOS header, even when they happen to match.
            send_type: match self.reliability {
                Reliability::UnreliableUnordered => -1,
                Reliability::ReliableUnordered => -2,
                Reliability::ReliableOrdered => -3,
            },
            payload,
        })
    }
}

pub fn decode(packet: &GamePacket) -> Option<Packet<'_>> {
    let reliability = match packet.send_type {
        -1 => Reliability::UnreliableUnordered,
        -2 => Reliability::ReliableUnordered,
        -3 => Reliability::ReliableOrdered,
        _ => return None,
    };
    let (&len, rest) = packet.payload.split_first()?;
    let (socket, data) = rest.split_at_checked(usize::from(len))?;
    let socket = std::str::from_utf8(socket).ok()?;
    if packet.peer == 0 || !valid(socket, data) {
        return None;
    }
    Some(Packet {
        socket,
        channel: u8::try_from(packet.channel).ok()?,
        reliability,
        data,
    })
}

fn valid(socket: &str, data: &[u8]) -> bool {
    (1..=MAX_SOCKET_NAME).contains(&socket.len())
        && socket
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_ +=.".contains(&b))
        && data.len() <= MAX_PACKET
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn steam_packets_and_incomplete_socket_names_are_not_delivered_as_eos() {
        let mut packet = Packet {
            socket: "Game",
            channel: 2,
            reliability: Reliability::UnreliableUnordered,
            data: b"",
        }
        .encode(1, 1)
        .unwrap();
        assert_eq!(decode(&packet).unwrap().data, b"");
        packet.send_type = 0;
        assert!(decode(&packet).is_none());
        packet.send_type = -1;
        packet.payload.truncate(2);
        assert!(decode(&packet).is_none());
    }
}
