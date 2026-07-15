use super::*;
use std::collections::HashMap;

#[derive(Debug)]
struct LanDropIncident {
    started_at: Instant,
    dropped_packets: u64,
}

#[derive(Debug, Default)]
struct LanDropIncidents {
    by_peer: HashMap<u64, LanDropIncident>,
}

impl LanDropIncidents {
    fn record_drop(&mut self, peer: u64, now: Instant) -> bool {
        if let Some(incident) = self.by_peer.get_mut(&peer) {
            incident.dropped_packets = incident.dropped_packets.saturating_add(1);
            return false;
        }

        self.by_peer.insert(
            peer,
            LanDropIncident {
                started_at: now,
                dropped_packets: 1,
            },
        );
        true
    }

    fn record_recovery(&mut self, peer: u64, now: Instant) -> Option<(Duration, u64)> {
        let incident = self.by_peer.remove(&peer)?;
        Some((
            now.saturating_duration_since(incident.started_at),
            incident.dropped_packets,
        ))
    }
}

pub(super) async fn lan_route_task(
    room: Arc<super::super::LanControlPlane>,
    mut outbound_rx: TokioReceiver<OutboundGamePacket>,
    event_tx: RuntimeEventSender,
    cancellation: CancellationToken,
    health: Option<SharedSessionHealth>,
) -> io::Result<()> {
    let mut observer = PacketObserver::default();
    let mut drop_incidents = LanDropIncidents::default();
    loop {
        tokio::select! {
            () = cancellation.cancelled() => {
                room.stop().await;
                return Ok(());
            }
            Some(packet) = outbound_rx.recv() => {
                let started = Instant::now();
                let sent_bytes = u64::try_from(packet.payload.len()).unwrap_or(u64::MAX);
                let peer = packet.to_steam_id64;
                let summary = PacketSummary {
                    peer,
                    sequence: packet.source_sequence,
                    source_sequence: packet.source_sequence,
                    channel: packet.channel,
                    send_type: packet.send_type,
                    payload_bytes: packet.payload.len(),
                    wire_bytes: tractor_beam_direct_protocol::DATA_FRAME_OVERHEAD
                        + packet.payload.len(),
                };
                match room.send_game(packet).await {
                    Ok(()) => {
                        observe_health(&health, |health| {
                            health.observe_network_send_duration(started.elapsed());
                        });
                        send_event(
                            &event_tx,
                            RuntimeEvent::CounterDelta(network_out_counter(sent_bytes)),
                        );
                        observer.observe_hook_packet(&event_tx, &summary);
                        if let Some((outage, dropped_packets)) =
                            drop_incidents.record_recovery(peer, Instant::now())
                        {
                            send_event(
                                &event_tx,
                                log_event(
                                    LogLevel::Info,
                                    format!(
                                        "Direct LAN peer path recovered outage_ms={} packets_dropped={dropped_packets}",
                                        outage.as_millis()
                                    ),
                                ),
                            );
                        }
                    }
                    Err(LanGameSendError::Unavailable(peer)) => {
                        observe_health(&health, SessionHealth::observe_network_send_drop);
                        if drop_incidents.record_drop(peer, Instant::now()) {
                            send_error(
                                &event_tx,
                                "Direct LAN peer path is unavailable; dropping gameplay packets",
                            );
                        }
                    }
                    Err(error) => {
                        send_error(&event_tx, format!("Direct LAN packet dropped: {error}"));
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lan_drop_incident_logs_once_until_the_peer_recovers() {
        let start = Instant::now();
        let mut incidents = LanDropIncidents::default();

        assert!(incidents.record_drop(42, start));
        assert!(!incidents.record_drop(42, start + Duration::from_millis(10)));
        assert_eq!(
            incidents.record_recovery(42, start + Duration::from_millis(25)),
            Some((Duration::from_millis(25), 2))
        );
        assert!(incidents.record_drop(42, start + Duration::from_millis(30)));
    }
}
