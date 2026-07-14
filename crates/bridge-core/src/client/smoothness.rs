use std::time::Duration;

use serde::Serialize;

use super::{
    QualityConfidence, RoomPathQualitySnapshot, RoomPathQualityState, SessionHealthSnapshot,
    SessionQuality, SessionQualityReason,
};

const WATCH_PATH_P95: Duration = Duration::from_millis(120);
const POOR_PATH_P95: Duration = Duration::from_millis(250);
const WATCH_JITTER: Duration = Duration::from_millis(30);
const POOR_JITTER: Duration = Duration::from_millis(80);
const WATCH_LOSS_BASIS_POINTS: u16 = 200;
const POOR_LOSS_BASIS_POINTS: u16 = 1_000;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SmoothnessReason {
    PathRttElevated,
    PathLoss,
    PathJitterElevated,
    LocalQueueDrop,
    SequenceGap,
    SequenceReordered,
    HookSendStall,
    RuntimeRttTimeout,
    StalePathSamples,
    InsufficientCurrentData,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct SmoothnessSnapshot {
    pub level: SessionQuality,
    pub confidence: QualityConfidence,
    pub observed_at: u64,
    pub freshness_seconds: Option<u64>,
    pub reasons: Vec<SmoothnessReason>,
    pub current_path_peers: u32,
    pub worst_current_path: Option<RoomPathQualitySnapshot>,
}

impl Default for SmoothnessSnapshot {
    fn default() -> Self {
        Self {
            level: SessionQuality::Unavailable,
            confidence: QualityConfidence::None,
            observed_at: 0,
            freshness_seconds: None,
            reasons: vec![SmoothnessReason::InsufficientCurrentData],
            current_path_peers: 0,
            worst_current_path: None,
        }
    }
}

pub(super) fn assess_smoothness(
    health: Option<&SessionHealthSnapshot>,
    paths: &[RoomPathQualitySnapshot],
    observed_at: u64,
) -> SmoothnessSnapshot {
    let current_paths = paths
        .iter()
        .filter(|path| path.state == RoomPathQualityState::Current && path.completed >= 5)
        .collect::<Vec<_>>();
    let worst_current_path = current_paths
        .iter()
        .copied()
        .max_by_key(|path| path_score(path))
        .copied();
    let mut reasons = health
        .into_iter()
        .flat_map(|health| health.reasons.iter().filter_map(map_local_reason))
        .collect::<Vec<_>>();
    let mut level = health.map_or(SessionQuality::Unavailable, |health| health.quality);
    let mut confidence = health.map_or(QualityConfidence::None, |health| health.confidence);
    let mut path_poor = false;
    let mut path_watch = false;

    for path in &current_paths {
        let missing = path.completed.saturating_sub(path.responses);
        let sustained = path.completed >= 10;
        if path.p95_rtt.is_some_and(|rtt| rtt >= WATCH_PATH_P95) {
            push_reason(&mut reasons, SmoothnessReason::PathRttElevated);
            path_watch = true;
            path_poor |= sustained && path.p95_rtt.is_some_and(|rtt| rtt >= POOR_PATH_P95);
        }
        if path
            .loss_basis_points
            .is_some_and(|loss| loss >= WATCH_LOSS_BASIS_POINTS && missing >= 2)
        {
            push_reason(&mut reasons, SmoothnessReason::PathLoss);
            path_watch = true;
            path_poor |= sustained
                && missing >= 3
                && path
                    .loss_basis_points
                    .is_some_and(|loss| loss >= POOR_LOSS_BASIS_POINTS);
        }
        if path.jitter.is_some_and(|jitter| jitter >= WATCH_JITTER) {
            push_reason(&mut reasons, SmoothnessReason::PathJitterElevated);
            path_watch = true;
            path_poor |= sustained && path.jitter.is_some_and(|jitter| jitter >= POOR_JITTER);
        }
    }

    if path_poor || level == SessionQuality::Poor {
        level = SessionQuality::Poor;
    } else if path_watch || level == SessionQuality::Watch {
        level = SessionQuality::Watch;
    } else if !current_paths.is_empty() || level == SessionQuality::Good {
        level = SessionQuality::Good;
    }

    let max_path_samples = current_paths
        .iter()
        .map(|path| path.completed)
        .max()
        .unwrap_or(0);
    confidence = max_confidence(confidence, path_confidence(max_path_samples));

    if level == SessionQuality::Unavailable {
        confidence = QualityConfidence::None;
        reasons.clear();
        reasons.push(
            if paths
                .iter()
                .any(|path| path.state == RoomPathQualityState::Stale)
            {
                SmoothnessReason::StalePathSamples
            } else {
                SmoothnessReason::InsufficientCurrentData
            },
        );
    } else {
        reasons.sort_unstable();
        reasons.dedup();
    }

    SmoothnessSnapshot {
        level,
        confidence,
        observed_at,
        freshness_seconds: current_paths
            .iter()
            .filter_map(|path| path.freshness.map(|age| age.as_secs()))
            .max(),
        reasons,
        current_path_peers: u32::try_from(current_paths.len()).unwrap_or(u32::MAX),
        worst_current_path,
    }
}

const fn path_confidence(samples: u32) -> QualityConfidence {
    if samples >= 20 {
        QualityConfidence::High
    } else if samples >= 10 {
        QualityConfidence::Medium
    } else if samples >= 5 {
        QualityConfidence::Low
    } else {
        QualityConfidence::None
    }
}

fn path_score(path: &RoomPathQualitySnapshot) -> (u16, u128, u128) {
    (
        path.loss_basis_points.unwrap_or(0),
        path.p95_rtt.unwrap_or_default().as_millis(),
        path.jitter.unwrap_or_default().as_millis(),
    )
}

fn map_local_reason(reason: &SessionQualityReason) -> Option<SmoothnessReason> {
    match reason {
        SessionQualityReason::LocalQueueDrop => Some(SmoothnessReason::LocalQueueDrop),
        SessionQualityReason::SequenceGap => Some(SmoothnessReason::SequenceGap),
        SessionQualityReason::SequenceReordered => Some(SmoothnessReason::SequenceReordered),
        SessionQualityReason::HookSendStall => Some(SmoothnessReason::HookSendStall),
        SessionQualityReason::RuntimeRttTimeout => Some(SmoothnessReason::RuntimeRttTimeout),
        SessionQualityReason::StartupOrIdle => None,
    }
}

fn push_reason(reasons: &mut Vec<SmoothnessReason>, reason: SmoothnessReason) {
    if !reasons.contains(&reason) {
        reasons.push(reason);
    }
}

const fn max_confidence(left: QualityConfidence, right: QualityConfidence) -> QualityConfidence {
    use QualityConfidence::{High, Low, Medium, None};
    match (left, right) {
        (High, _) | (_, High) => High,
        (Medium, _) | (_, Medium) => Medium,
        (Low, _) | (_, Low) => Low,
        (None, None) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stale_or_insufficient_evidence_is_unavailable() {
        let stale = path(RoomPathQualityState::Stale, 30, 30, 20, 0, 5);
        let estimate = assess_smoothness(None, &[stale], 10);
        assert_eq!(estimate.level, SessionQuality::Unavailable);
        assert_eq!(estimate.confidence, QualityConfidence::None);
        assert_eq!(estimate.reasons, [SmoothnessReason::StalePathSamples]);
    }

    #[test]
    fn sustained_path_degradation_is_poor_but_one_loss_is_not() {
        let isolated = path(RoomPathQualityState::Current, 30, 29, 40, 334, 5);
        assert_eq!(
            assess_smoothness(None, &[isolated], 10).level,
            SessionQuality::Good
        );

        let degraded = path(RoomPathQualityState::Current, 30, 24, 300, 2_000, 90);
        let estimate = assess_smoothness(None, &[degraded], 10);
        assert_eq!(estimate.level, SessionQuality::Poor);
        assert_eq!(estimate.confidence, QualityConfidence::High);
        assert_eq!(
            estimate.reasons,
            [
                SmoothnessReason::PathRttElevated,
                SmoothnessReason::PathLoss,
                SmoothnessReason::PathJitterElevated,
            ]
        );
    }

    fn path(
        state: RoomPathQualityState,
        completed: u32,
        responses: u32,
        p95_ms: u64,
        loss_basis_points: u16,
        jitter_ms: u64,
    ) -> RoomPathQualitySnapshot {
        RoomPathQualitySnapshot {
            state,
            completed,
            responses,
            p95_rtt: Some(Duration::from_millis(p95_ms)),
            jitter: Some(Duration::from_millis(jitter_ms)),
            loss_basis_points: Some(loss_basis_points),
            freshness: Some(Duration::from_secs(1)),
            ..RoomPathQualitySnapshot::default()
        }
    }
}
