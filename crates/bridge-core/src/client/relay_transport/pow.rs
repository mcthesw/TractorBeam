use super::*;

pub(super) fn solve_pow(
    challenge_id: &str,
    session: &[u8; 16],
    steam_id64: u64,
    nonce: &str,
    difficulty_bits: u8,
) -> io::Result<String> {
    if difficulty_bits == 0 {
        return Ok(String::new());
    }
    let challenge = decode_hex_16(challenge_id)?;
    let nonce = decode_hex_16(nonce)?;
    for counter in 0_u64.. {
        let proof = format!("{counter:016x}");
        let mut hasher = Sha256::new();
        hasher.update(challenge);
        hasher.update(session);
        hasher.update(steam_id64.to_be_bytes());
        hasher.update(nonce);
        hasher.update(proof.as_bytes());
        let digest: [u8; 32] = hasher.finalize().into();
        if leading_zero_bits(&digest, difficulty_bits) {
            return Ok(proof);
        }
    }
    Err(io::Error::other("proof-of-work search exhausted"))
}

pub(super) fn decode_hex_16(value: &str) -> io::Result<[u8; 16]> {
    if value.len() != 32 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid challenge length",
        ));
    }
    let mut bytes = [0_u8; 16];
    for (index, pair) in value.as_bytes().chunks_exact(2).enumerate() {
        let pair = std::str::from_utf8(pair).map_err(io::Error::other)?;
        bytes[index] = u8::from_str_radix(pair, 16).map_err(io::Error::other)?;
    }
    Ok(bytes)
}

pub(super) fn leading_zero_bits(bytes: &[u8; 32], bits: u8) -> bool {
    let whole = usize::from(bits / 8);
    let rest = bits % 8;
    whole <= bytes.len()
        && bytes[..whole].iter().all(|byte| *byte == 0)
        && (rest == 0 || bytes.get(whole).is_some_and(|byte| byte >> (8 - rest) == 0))
}
