use std::time::{Duration, Instant};

use bytes::Bytes;

use super::*;

#[test]
fn fec_roundtrip_recovers_one_missing_original() {
    let profile = UdpFecProfile::for_name(UdpFecProfileName::Rs8_2_4ms);
    let now = Instant::now();
    let mut encoder = UdpFecEncoder::new(profile);
    let mut frames = Vec::new();
    for value in 0..4_u8 {
        frames.extend(
            encoder
                .encode(Bytes::from(vec![value; 32]), now)
                .expect("encode"),
        );
    }
    frames.extend(encoder.flush_pending().expect("flush"));
    frames.retain(|frame| {
        let decoded = UdpFecFrame::decode(frame.clone()).expect("frame");
        !(decoded.kind == KIND_ORIGINAL && decoded.shard_index == 2)
    });

    let mut decoder = UdpFecDecoder::new(profile);
    let mut recovered = Vec::new();
    for frame in frames {
        recovered.extend(
            decoder
                .decode(frame, now + Duration::from_millis(1))
                .unwrap(),
        );
    }

    assert!(recovered.iter().any(|payload| payload.as_ref() == [2; 32]));
    assert_eq!(decoder.snapshot().recovered_packets, 1);
}

#[test]
fn fec_rejects_oversized_inner_datagram() {
    let profile = UdpFecProfile::for_name(UdpFecProfileName::Rs8_2_4ms);
    let mut encoder = UdpFecEncoder::new(profile);

    let error = encoder
        .encode(
            Bytes::from(vec![0; profile.max_inner_bytes + 1]),
            Instant::now(),
        )
        .unwrap_err();

    assert!(matches!(error, UdpFecError::InnerDatagramTooLarge { .. }));
}

#[test]
fn oversized_datagram_can_passthrough_without_fec_wrapping() {
    let profile = UdpFecProfile::for_name(UdpFecProfileName::Rs8_2_4ms);
    let mut encoder = UdpFecEncoder::new(profile);
    let payload = Bytes::from(vec![0; profile.max_inner_bytes + 1]);

    let frames = encoder
        .encode_or_passthrough(payload.clone(), Instant::now())
        .unwrap();

    assert_eq!(frames, vec![payload]);
    assert_eq!(encoder.snapshot().oversized_passthrough_packets, 1);
}

#[test]
fn low_flow_group_flushes_at_profile_deadline() {
    let profile = UdpFecProfile::for_name(UdpFecProfileName::Rs8_2_4ms);
    let now = Instant::now();
    let mut encoder = UdpFecEncoder::new(profile);

    encoder
        .encode(Bytes::from_static(b"one-small-packet"), now)
        .unwrap();

    assert!(
        encoder
            .flush_expired(now + Duration::from_millis(3))
            .unwrap()
            .is_empty()
    );
    assert!(
        !encoder
            .flush_expired(now + Duration::from_millis(4))
            .unwrap()
            .is_empty()
    );
}

#[test]
fn non_fec_frames_pass_through_decoder() {
    let profile = UdpFecProfile::for_name(UdpFecProfileName::Rs8_2_4ms);
    let mut decoder = UdpFecDecoder::new(profile);
    let payload = Bytes::from_static(b"plain");

    assert_eq!(
        decoder.decode(payload.clone(), Instant::now()).unwrap(),
        vec![payload]
    );
}
