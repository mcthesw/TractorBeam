use std::{
    collections::{HashMap, VecDeque},
    fmt::{self, Display},
    time::{Duration, Instant},
};

use bytes::{Buf, BufMut, Bytes, BytesMut};
use reed_solomon_simd::{ReedSolomonDecoder, ReedSolomonEncoder};
use serde::{Deserialize, Serialize};
use thiserror::Error;

const FEC_MAGIC: &[u8; 4] = b"BBF1";
const FEC_VERSION: u8 = 1;
const KIND_ORIGINAL: u8 = 0;
const KIND_REPAIR: u8 = 1;
const MAX_PROFILE_DATA_SHARDS: usize = 8;
const HEADER_LEN: usize = 30;
const PROFILE_RS_8_2_4MS_ID: u8 = 1;

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub enum UdpFecProfileName {
    #[default]
    #[serde(rename = "rs_8_2_4ms")]
    Rs8_2_4ms,
}

impl UdpFecProfileName {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Rs8_2_4ms => "rs_8_2_4ms",
        }
    }
}

impl Display for UdpFecProfileName {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct UdpFecConfig {
    pub enabled: bool,
    pub profile: UdpFecProfileName,
}

impl Default for UdpFecConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            profile: UdpFecProfileName::Rs8_2_4ms,
        }
    }
}

impl UdpFecConfig {
    #[must_use]
    pub fn active_profile(self) -> Option<UdpFecProfile> {
        self.enabled.then(|| UdpFecProfile::for_name(self.profile))
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub struct UdpFecProfile {
    pub name: UdpFecProfileName,
    pub data_shards: usize,
    pub repair_shards: usize,
    pub flush_after: Duration,
    pub max_inner_bytes: usize,
}

impl UdpFecProfile {
    #[must_use]
    pub const fn for_name(name: UdpFecProfileName) -> Self {
        match name {
            UdpFecProfileName::Rs8_2_4ms => Self {
                name,
                data_shards: 8,
                repair_shards: 2,
                flush_after: Duration::from_millis(4),
                max_inner_bytes: 1_170,
            },
        }
    }

    const fn profile_id(self) -> u8 {
        match self.name {
            UdpFecProfileName::Rs8_2_4ms => PROFILE_RS_8_2_4MS_ID,
        }
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
pub struct UdpFecSnapshot {
    pub profile: Option<String>,
    pub original_packets: u64,
    pub repair_packets: u64,
    pub recovered_packets: u64,
    pub unrecovered_groups: u64,
    pub oversized_passthrough_packets: u64,
    pub decode_delay_p95_ms: Option<u64>,
    pub profile_epoch: u64,
    pub encoded_bytes: u64,
    pub repair_bytes: u64,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
pub struct UdpFecSessionSnapshot {
    pub send: Option<UdpFecSnapshot>,
    pub receive: Option<UdpFecSnapshot>,
}

impl UdpFecSessionSnapshot {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.send.is_none() && self.receive.is_none()
    }

    #[must_use]
    pub fn compact_log_line(&self, label: &str) -> String {
        let send = self
            .send
            .as_ref()
            .map(snapshot_summary)
            .unwrap_or_else(|| "send=none".to_owned());
        let receive = self
            .receive
            .as_ref()
            .map(snapshot_summary)
            .unwrap_or_else(|| "receive=none".to_owned());
        format!("{label}: {send} {receive}")
    }
}

fn snapshot_summary(snapshot: &UdpFecSnapshot) -> String {
    format!(
        "profile={} orig={} repair={} recovered={} unrecovered={} oversized_passthrough={} delay_p95={} overhead_bytes={}",
        snapshot.profile.as_deref().unwrap_or("none"),
        snapshot.original_packets,
        snapshot.repair_packets,
        snapshot.recovered_packets,
        snapshot.unrecovered_groups,
        snapshot.oversized_passthrough_packets,
        snapshot
            .decode_delay_p95_ms
            .map_or_else(|| "none".to_owned(), |value| value.to_string()),
        snapshot.repair_bytes,
    )
}

#[derive(Clone, Debug, Default)]
struct DecodeDelay {
    samples: Vec<u64>,
}

impl DecodeDelay {
    fn observe(&mut self, duration: Duration) {
        self.samples
            .push(u64::try_from(duration.as_millis()).unwrap_or(u64::MAX));
    }

    fn p95_ms(&self) -> Option<u64> {
        if self.samples.is_empty() {
            return None;
        }
        let mut samples = self.samples.clone();
        samples.sort_unstable();
        let index = (samples.len().saturating_sub(1) * 95 + 50) / 100;
        samples.get(index).copied()
    }
}

#[derive(Clone, Debug, Default)]
struct UdpFecCounters {
    original_packets: u64,
    repair_packets: u64,
    recovered_packets: u64,
    unrecovered_groups: u64,
    oversized_passthrough_packets: u64,
    profile_epoch: u64,
    encoded_bytes: u64,
    repair_bytes: u64,
    decode_delay: DecodeDelay,
}

impl UdpFecCounters {
    fn snapshot(&self, profile: Option<UdpFecProfile>) -> UdpFecSnapshot {
        UdpFecSnapshot {
            profile: profile.map(|profile| profile.name.to_string()),
            original_packets: self.original_packets,
            repair_packets: self.repair_packets,
            recovered_packets: self.recovered_packets,
            unrecovered_groups: self.unrecovered_groups,
            oversized_passthrough_packets: self.oversized_passthrough_packets,
            decode_delay_p95_ms: self.decode_delay.p95_ms(),
            profile_epoch: self.profile_epoch,
            encoded_bytes: self.encoded_bytes,
            repair_bytes: self.repair_bytes,
        }
    }
}

#[derive(Debug, Error)]
pub enum UdpFecError {
    #[error("UDP FEC frame is too short")]
    ShortFrame,
    #[error("UDP FEC frame magic did not match")]
    BadMagic,
    #[error("unsupported UDP FEC frame version: {0}")]
    UnsupportedVersion(u8),
    #[error("unsupported UDP FEC profile id: {0}")]
    UnsupportedProfile(u8),
    #[error("invalid UDP FEC shard layout")]
    InvalidShardLayout,
    #[error("UDP FEC inner datagram exceeds profile limit: {got} > {max}")]
    InnerDatagramTooLarge { got: usize, max: usize },
    #[error("reed-solomon error: {0}")]
    ReedSolomon(#[from] reed_solomon_simd::Error),
}

#[derive(Clone, Debug)]
struct PendingOriginal {
    padded: Vec<u8>,
    len: usize,
}

#[derive(Debug)]
pub struct UdpFecEncoder {
    profile: UdpFecProfile,
    next_group_id: u32,
    pending_group_id: u32,
    pending_started: Option<Instant>,
    pending: Vec<PendingOriginal>,
    counters: UdpFecCounters,
}

impl UdpFecEncoder {
    #[must_use]
    pub fn new(profile: UdpFecProfile) -> Self {
        Self {
            profile,
            next_group_id: 1,
            pending_group_id: 1,
            pending_started: None,
            pending: Vec::with_capacity(profile.data_shards),
            counters: UdpFecCounters {
                profile_epoch: 1,
                ..UdpFecCounters::default()
            },
        }
    }

    pub fn encode(&mut self, payload: Bytes, now: Instant) -> Result<Vec<Bytes>, UdpFecError> {
        if payload.len() > self.profile.max_inner_bytes {
            return Err(UdpFecError::InnerDatagramTooLarge {
                got: payload.len(),
                max: self.profile.max_inner_bytes,
            });
        }
        let mut frames = self.flush_expired(now)?;
        if self.pending.is_empty() {
            self.pending_group_id = self.next_group_id;
            self.next_group_id = self.next_group_id.wrapping_add(1);
            self.pending_started = Some(now);
        }
        let shard_index = self.pending.len();
        let mut padded = payload.to_vec();
        padded.resize(self.profile.max_inner_bytes, 0);
        self.pending.push(PendingOriginal {
            padded,
            len: payload.len(),
        });
        self.counters.original_packets = self.counters.original_packets.saturating_add(1);
        self.counters.encoded_bytes = self
            .counters
            .encoded_bytes
            .saturating_add(u64::try_from(payload.len()).unwrap_or(u64::MAX));
        frames.push(original_frame(
            self.profile,
            self.pending_group_id,
            shard_index,
            &self.pending,
        ));
        if self.pending.len() == self.profile.data_shards {
            frames.extend(self.flush_pending()?);
        }
        Ok(frames)
    }

    pub fn encode_or_passthrough(
        &mut self,
        payload: Bytes,
        now: Instant,
    ) -> Result<Vec<Bytes>, UdpFecError> {
        if payload.len() > self.profile.max_inner_bytes {
            self.counters.oversized_passthrough_packets = self
                .counters
                .oversized_passthrough_packets
                .saturating_add(1);
            return Ok(vec![payload]);
        }
        self.encode(payload, now)
    }

    pub fn flush_expired(&mut self, now: Instant) -> Result<Vec<Bytes>, UdpFecError> {
        if self
            .pending_started
            .is_some_and(|started| now.duration_since(started) >= self.profile.flush_after)
        {
            return self.flush_pending();
        }
        Ok(Vec::new())
    }

    pub fn flush_pending(&mut self) -> Result<Vec<Bytes>, UdpFecError> {
        if self.pending.is_empty() {
            return Ok(Vec::new());
        }
        let group_id = self.pending_group_id;
        let pending = std::mem::take(&mut self.pending);
        self.pending_started = None;
        let originals = pending.iter().map(|original| original.padded.as_slice());
        let mut encoder = ReedSolomonEncoder::new(
            pending.len(),
            self.profile.repair_shards,
            self.profile.max_inner_bytes,
        )?;
        for original in originals {
            encoder.add_original_shard(original)?;
        }
        let encoded = encoder.encode()?;
        let frames = encoded
            .recovery_iter()
            .enumerate()
            .map(|(index, shard)| {
                self.counters.repair_packets = self.counters.repair_packets.saturating_add(1);
                self.counters.repair_bytes = self
                    .counters
                    .repair_bytes
                    .saturating_add(u64::try_from(shard.len()).unwrap_or(u64::MAX));
                repair_frame(self.profile, group_id, index, &pending, shard)
            })
            .collect();
        Ok(frames)
    }

    #[must_use]
    pub fn snapshot(&self) -> UdpFecSnapshot {
        self.counters.snapshot(Some(self.profile))
    }
}

#[derive(Clone, Debug)]
struct DecodeGroup {
    created_at: Instant,
    data_count: usize,
    repair_count: usize,
    lengths: [usize; MAX_PROFILE_DATA_SHARDS],
    originals: Vec<Option<Bytes>>,
    repairs: Vec<Option<Bytes>>,
    delivered: Vec<bool>,
}

impl DecodeGroup {
    fn new(frame: &UdpFecFrame, now: Instant) -> Self {
        Self {
            created_at: now,
            data_count: frame.data_count,
            repair_count: frame.repair_count,
            lengths: frame.lengths,
            originals: vec![None; frame.data_count],
            repairs: vec![None; frame.repair_count],
            delivered: vec![false; frame.data_count],
        }
    }

    fn merge_layout(&mut self, frame: &UdpFecFrame) -> Result<(), UdpFecError> {
        if frame.data_count < self.data_count || frame.repair_count < self.repair_count {
            return Err(UdpFecError::InvalidShardLayout);
        }
        if frame.data_count > self.data_count {
            self.originals.resize(frame.data_count, None);
            self.delivered.resize(frame.data_count, false);
            self.data_count = frame.data_count;
        }
        if frame.repair_count > self.repair_count {
            self.repairs.resize(frame.repair_count, None);
            self.repair_count = frame.repair_count;
        }
        for (index, length) in frame.lengths.iter().copied().enumerate() {
            if length > 0 {
                self.lengths[index] = length;
            }
        }
        Ok(())
    }
}

#[derive(Debug)]
pub struct UdpFecDecoder {
    profile: UdpFecProfile,
    groups: HashMap<u32, DecodeGroup>,
    order: VecDeque<u32>,
    counters: UdpFecCounters,
}

impl UdpFecDecoder {
    #[must_use]
    pub fn new(profile: UdpFecProfile) -> Self {
        Self {
            profile,
            groups: HashMap::new(),
            order: VecDeque::new(),
            counters: UdpFecCounters {
                profile_epoch: 1,
                ..UdpFecCounters::default()
            },
        }
    }

    pub fn decode(&mut self, frame: Bytes, now: Instant) -> Result<Vec<Bytes>, UdpFecError> {
        if !is_fec_frame(&frame) {
            return Ok(vec![frame]);
        }
        let frame = UdpFecFrame::decode(frame)?;
        if frame.profile_id != self.profile.profile_id() {
            return Err(UdpFecError::UnsupportedProfile(frame.profile_id));
        }
        let group = self.groups.entry(frame.group_id).or_insert_with(|| {
            self.order.push_back(frame.group_id);
            DecodeGroup::new(&frame, now)
        });
        group.merge_layout(&frame)?;
        match frame.kind {
            KIND_ORIGINAL => {
                if frame.shard_index >= group.data_count {
                    return Err(UdpFecError::InvalidShardLayout);
                }
                let output = trim_original(&frame.shard, group.lengths[frame.shard_index]);
                group.originals[frame.shard_index] = Some(frame.shard);
                if !group.delivered[frame.shard_index] {
                    group.delivered[frame.shard_index] = true;
                    self.counters.original_packets =
                        self.counters.original_packets.saturating_add(1);
                    return Ok(vec![output]);
                }
                Ok(Vec::new())
            }
            KIND_REPAIR => {
                if frame.shard_index >= group.repair_count {
                    return Err(UdpFecError::InvalidShardLayout);
                }
                if group.repairs[frame.shard_index].is_none() {
                    self.counters.repair_packets = self.counters.repair_packets.saturating_add(1);
                }
                group.repairs[frame.shard_index] = Some(frame.shard);
                let restored = try_restore_group(group)?;
                let mut output = Vec::new();
                for (index, payload) in restored {
                    if !group.delivered[index] {
                        group.delivered[index] = true;
                        self.counters.recovered_packets =
                            self.counters.recovered_packets.saturating_add(1);
                        self.counters
                            .decode_delay
                            .observe(now.duration_since(group.created_at));
                        output.push(payload);
                    }
                }
                Ok(output)
            }
            _ => Err(UdpFecError::InvalidShardLayout),
        }
    }

    pub fn expire(&mut self, now: Instant) {
        while let Some(group_id) = self.order.front().copied() {
            let Some(group) = self.groups.get(&group_id) else {
                self.order.pop_front();
                continue;
            };
            if now.duration_since(group.created_at) <= self.profile.flush_after * 16 {
                break;
            }
            let missing = group.delivered.iter().any(|delivered| !delivered);
            if missing {
                self.counters.unrecovered_groups =
                    self.counters.unrecovered_groups.saturating_add(1);
            }
            self.groups.remove(&group_id);
            self.order.pop_front();
        }
    }

    #[must_use]
    pub fn snapshot(&self) -> UdpFecSnapshot {
        self.counters.snapshot(Some(self.profile))
    }
}

fn is_fec_frame(bytes: &[u8]) -> bool {
    bytes.len() >= FEC_MAGIC.len() && &bytes[..FEC_MAGIC.len()] == FEC_MAGIC
}

fn original_frame(
    profile: UdpFecProfile,
    group_id: u32,
    shard_index: usize,
    pending: &[PendingOriginal],
) -> Bytes {
    encode_frame(
        profile,
        KIND_ORIGINAL,
        group_id,
        shard_index,
        pending,
        &pending[shard_index].padded,
    )
}

fn repair_frame(
    profile: UdpFecProfile,
    group_id: u32,
    shard_index: usize,
    pending: &[PendingOriginal],
    shard: &[u8],
) -> Bytes {
    encode_frame(profile, KIND_REPAIR, group_id, shard_index, pending, shard)
}

fn encode_frame(
    profile: UdpFecProfile,
    kind: u8,
    group_id: u32,
    shard_index: usize,
    pending: &[PendingOriginal],
    shard: &[u8],
) -> Bytes {
    let mut frame = BytesMut::with_capacity(HEADER_LEN + shard.len());
    frame.extend_from_slice(FEC_MAGIC);
    frame.put_u8(FEC_VERSION);
    frame.put_u8(kind);
    frame.put_u32(group_id);
    frame.put_u8(u8::try_from(shard_index).unwrap_or(u8::MAX));
    frame.put_u8(u8::try_from(pending.len()).unwrap_or(u8::MAX));
    frame.put_u8(u8::try_from(profile.repair_shards).unwrap_or(u8::MAX));
    frame.put_u8(profile.profile_id());
    for index in 0..MAX_PROFILE_DATA_SHARDS {
        let len = pending.get(index).map_or(0, |original| original.len);
        frame.put_u16(u16::try_from(len).unwrap_or(u16::MAX));
    }
    frame.extend_from_slice(shard);
    frame.freeze()
}

#[derive(Clone, Debug)]
struct UdpFecFrame {
    kind: u8,
    group_id: u32,
    shard_index: usize,
    data_count: usize,
    repair_count: usize,
    profile_id: u8,
    lengths: [usize; MAX_PROFILE_DATA_SHARDS],
    shard: Bytes,
}

impl UdpFecFrame {
    fn decode(mut bytes: Bytes) -> Result<Self, UdpFecError> {
        if bytes.len() < HEADER_LEN {
            return Err(UdpFecError::ShortFrame);
        }
        if &bytes[..FEC_MAGIC.len()] != FEC_MAGIC {
            return Err(UdpFecError::BadMagic);
        }
        bytes.advance(FEC_MAGIC.len());
        let version = bytes.get_u8();
        if version != FEC_VERSION {
            return Err(UdpFecError::UnsupportedVersion(version));
        }
        let kind = bytes.get_u8();
        let group_id = bytes.get_u32();
        let shard_index = usize::from(bytes.get_u8());
        let data_count = usize::from(bytes.get_u8());
        let repair_count = usize::from(bytes.get_u8());
        let profile_id = bytes.get_u8();
        if data_count == 0
            || data_count > MAX_PROFILE_DATA_SHARDS
            || repair_count == 0
            || (kind == KIND_ORIGINAL && shard_index >= data_count)
            || (kind == KIND_REPAIR && shard_index >= repair_count)
        {
            return Err(UdpFecError::InvalidShardLayout);
        }
        let mut lengths = [0; MAX_PROFILE_DATA_SHARDS];
        for length in lengths.iter_mut().take(MAX_PROFILE_DATA_SHARDS) {
            *length = usize::from(bytes.get_u16());
        }
        Ok(Self {
            kind,
            group_id,
            shard_index,
            data_count,
            repair_count,
            profile_id,
            lengths,
            shard: bytes,
        })
    }
}

fn trim_original(shard: &[u8], len: usize) -> Bytes {
    Bytes::copy_from_slice(&shard[..len.min(shard.len())])
}

fn try_restore_group(group: &DecodeGroup) -> Result<Vec<(usize, Bytes)>, UdpFecError> {
    let original_count = group
        .originals
        .iter()
        .filter(|shard| shard.is_some())
        .count();
    let repair_count = group.repairs.iter().filter(|shard| shard.is_some()).count();
    if original_count == group.data_count {
        return Ok(Vec::new());
    }
    if original_count + repair_count < group.data_count {
        return Ok(Vec::new());
    }
    let mut decoder = ReedSolomonDecoder::new(
        group.data_count,
        group.repair_count,
        group.originals[0]
            .as_ref()
            .or_else(|| group.repairs.iter().flatten().next())
            .map_or(0, Bytes::len),
    )?;
    for (index, shard) in group.originals.iter().enumerate() {
        if let Some(shard) = shard {
            decoder.add_original_shard(index, shard)?;
        }
    }
    for (index, shard) in group.repairs.iter().enumerate() {
        if let Some(shard) = shard {
            decoder.add_recovery_shard(index, shard)?;
        }
    }
    let result = decoder.decode()?;
    Ok(result
        .restored_original_iter()
        .map(|(index, shard)| (index, trim_original(shard, group.lengths[index])))
        .collect())
}

#[cfg(test)]
#[path = "udp_fec_tests.rs"]
mod tests;
