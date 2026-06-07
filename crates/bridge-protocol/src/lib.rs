//! Packet protocol shared by the Basement Bridge client, sidecar, and relay.

/// Wire protocol version used by the validated prototype.
pub const PROTOCOL_VERSION: u8 = 1;

/// Local hook-to-sidecar frame magic.
pub const LOCAL_MAGIC: &[u8; 4] = b"IBR1";

/// Sidecar-to-relay frame magic.
pub const RELAY_MAGIC: &[u8; 4] = b"IBR2";

/// Returns true when a frame magic is one of the known bridge magics.
pub fn is_known_magic(value: &[u8; 4]) -> bool {
    value == LOCAL_MAGIC || value == RELAY_MAGIC
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_bridge_magics() {
        assert!(is_known_magic(LOCAL_MAGIC));
        assert!(is_known_magic(RELAY_MAGIC));
        assert!(!is_known_magic(b"NOPE"));
    }
}
