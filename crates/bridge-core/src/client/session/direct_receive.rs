use super::*;

#[derive(Debug)]
struct DirectReceiveIncident {
    started_at: Instant,
    dropped_packets: u64,
}

#[derive(Debug, Default)]
struct DirectReceiveState {
    closed: bool,
    incident: Option<DirectReceiveIncident>,
}

pub(super) struct DirectReceiveObserver {
    event_tx: RuntimeEventSender,
    health: Option<SharedSessionHealth>,
    state: Mutex<DirectReceiveState>,
}

impl DirectReceiveObserver {
    pub(super) fn new(event_tx: RuntimeEventSender, health: Option<SharedSessionHealth>) -> Self {
        observe_health(&health, SessionHealth::enable_direct_receive_handoff);
        Self {
            event_tx,
            health,
            state: Mutex::new(DirectReceiveState::default()),
        }
    }

    fn finish_incident(
        &self,
        now: Instant,
    ) -> Option<crate::client::lan::LanInboundHandoffIncidentSummary> {
        let incident = {
            let mut state = self
                .state
                .lock()
                .expect("Direct receive observer lock poisoned");
            state.closed = true;
            state.incident.take()
        };
        incident.map(
            |incident| crate::client::lan::LanInboundHandoffIncidentSummary {
                duration: now.saturating_duration_since(incident.started_at),
                dropped_packets: incident.dropped_packets,
            },
        )
    }
}

impl crate::client::lan::LanInboundHandoffObserver for DirectReceiveObserver {
    fn observe(&self, accepted: bool, now: Instant) {
        let mut state = self
            .state
            .lock()
            .expect("Direct receive observer lock poisoned");
        if state.closed {
            return;
        }
        observe_health(&self.health, |health| {
            health.observe_direct_receive_handoff(accepted);
        });

        if accepted {
            if let Some(incident) = state.incident.take() {
                send_event(
                    &self.event_tx,
                    log_event(
                        LogLevel::Info,
                        format!(
                            "Direct receive handoff recovered outage_ms={} packets_dropped={}",
                            now.saturating_duration_since(incident.started_at)
                                .as_millis(),
                            incident.dropped_packets,
                        ),
                    ),
                );
            }
            return;
        }

        if let Some(incident) = state.incident.as_mut() {
            incident.dropped_packets = incident.dropped_packets.saturating_add(1);
            return;
        }
        state.incident = Some(DirectReceiveIncident {
            started_at: now,
            dropped_packets: 1,
        });
        send_event(
            &self.event_tx,
            log_event(
                LogLevel::Warn,
                "Direct receive queue is full; dropping gameplay packets",
            ),
        );
    }

    fn finish(&self, now: Instant) -> Option<crate::client::lan::LanInboundHandoffIncidentSummary> {
        self.finish_incident(now)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::lan::LanInboundHandoffObserver as _;

    #[test]
    fn one_incident_emits_onset_and_recovery_without_packet_log_flood() {
        let start = Instant::now();
        let (event_tx, mut event_rx) = tokio_mpsc::channel(8);
        let health = Arc::new(Mutex::new(SessionHealth::new(
            false,
            Duration::from_secs(1),
            start,
        )));
        let observer = DirectReceiveObserver::new(event_tx, Some(health.clone()));

        observer.observe(false, start);
        observer.observe(false, start + Duration::from_millis(5));
        observer.observe(true, start + Duration::from_millis(12));

        let events = std::iter::from_fn(|| event_rx.try_recv().ok()).collect::<Vec<_>>();
        assert_eq!(events.len(), 2);
        assert!(matches!(
            &events[0],
            RuntimeEvent::Log(LogLevel::Warn, message)
                if message.contains("queue is full")
        ));
        assert!(matches!(
            &events[1],
            RuntimeEvent::Log(LogLevel::Info, message)
                if message.contains("outage_ms=12") && message.contains("packets_dropped=2")
        ));

        let snapshot = health
            .lock()
            .unwrap()
            .snapshot(start + Duration::from_millis(12));
        assert_eq!(snapshot.direct_receive.attempted, 3);
        assert_eq!(snapshot.direct_receive.dropped, 2);
        assert!(!snapshot.direct_receive.saturated);
    }

    #[test]
    fn finish_summarizes_an_unrecovered_incident_and_ignores_late_observations() {
        let start = Instant::now();
        let (event_tx, mut event_rx) = tokio_mpsc::channel(8);
        let health = Arc::new(Mutex::new(SessionHealth::new(
            false,
            Duration::from_secs(1),
            start,
        )));
        let observer = DirectReceiveObserver::new(event_tx, Some(health.clone()));

        observer.observe(false, start);
        let summary = observer
            .finish(start + Duration::from_millis(20))
            .expect("active incident should produce a final summary");
        observer.observe(false, start + Duration::from_millis(30));

        let events = std::iter::from_fn(|| event_rx.try_recv().ok()).collect::<Vec<_>>();
        assert_eq!(events.len(), 1);
        assert_eq!(summary.duration, Duration::from_millis(20));
        assert_eq!(summary.dropped_packets, 1);
        assert!(observer.finish(start + Duration::from_millis(40)).is_none());
        assert_eq!(
            health
                .lock()
                .unwrap()
                .snapshot(start + Duration::from_millis(30))
                .direct_receive
                .attempted,
            1
        );
    }
}
