use super::*;

const DIRECT_HOOK_RETRY_INTERVAL: Duration = Duration::from_millis(2);

struct PendingHookDelivery {
    packet: tractor_beam_hook_ipc::GamePacket,
    receipt: crate::client::lan::LanInboundReceipt,
    summary: PacketSummary,
    received_bytes: u64,
}

pub(super) struct DirectSendObserver {
    event_tx: RuntimeEventSender,
    health: Option<SharedSessionHealth>,
    packets: Mutex<PacketObserver>,
}

impl DirectSendObserver {
    pub(super) fn new(event_tx: RuntimeEventSender, health: Option<SharedSessionHealth>) -> Self {
        Self {
            event_tx,
            health,
            packets: Mutex::new(PacketObserver::default()),
        }
    }
}

impl crate::client::lan::LanGameSendObserver for DirectSendObserver {
    fn observe(&self, success: crate::client::lan::LanGameSendSuccess, duration: Duration) {
        observe_health(&self.health, |health| {
            health.observe_network_send_duration(duration);
        });
        send_event(
            &self.event_tx,
            RuntimeEvent::CounterDelta(network_out_counter(
                u64::try_from(success.payload_bytes).unwrap_or(u64::MAX),
            )),
        );
        let summary = PacketSummary {
            peer: success.peer,
            hook_sequence: success.hook_sequence,
            delivery_sequence: success.delivery_sequence,
            channel: success.channel,
            send_type: success.send_type,
            payload_bytes: success.payload_bytes,
            wire_bytes: success.wire_bytes,
        };
        if let Ok(mut packets) = self.packets.lock() {
            packets.observe_hook_packet(&self.event_tx, &summary);
        }
    }
}

pub(super) async fn direct_hook_in_task(
    room: Arc<super::super::LanControlPlane>,
    mut hook_packets_rx: TokioReceiver<tractor_beam_hook_ipc::GamePacket>,
    cancellation: CancellationToken,
    health: Option<SharedSessionHealth>,
) -> io::Result<()> {
    let mut delivery_streams = DeliveryStreamAllocator::default();
    loop {
        tokio::select! {
            () = cancellation.cancelled() => {
                room.stop().await;
                return Ok(());
            },
            packet = hook_packets_rx.recv() => {
                let Some(packet) = packet else {
                    room.stop().await;
                    return Ok(());
                };
                let size = packet.payload.len();
                observe_health(&health, |health| {
                    health.observe_hook_in_recv(size, Instant::now());
                });
                let packet = delivery_streams.assign_hook_packet(packet);
                let _ = room.try_send_game(packet);
            }
        }
    }
}

pub(super) async fn direct_hook_out_task(
    to_hook: hook_ipc::ClientIpcSender,
    mut inbound: crate::client::lan::LanInboundReceiver,
    event_tx: RuntimeEventSender,
    cancellation: CancellationToken,
    health: Option<SharedSessionHealth>,
) -> io::Result<()> {
    let mut local_sequence = 1_u32;
    let mut observer = PacketObserver::default();
    let mut pending = None;
    loop {
        if pending.is_none() {
            let delivery = tokio::select! {
                () = cancellation.cancelled() => return Ok(()),
                delivery = inbound.recv() => delivery,
            };
            let Some(delivery) = delivery else {
                return Ok(());
            };
            let from_steam_id64 = delivery.packet.from_steam_id64;
            let delivery_stream_id = delivery.packet.delivery_stream_id;
            let delivery_sequence = delivery.packet.delivery_sequence;
            let (packet, summary, received_bytes) =
                encode_inbound_hook_packet(delivery.packet, &mut local_sequence);
            observe_health(&health, |health| {
                health.observe_network_recv(summary.payload_bytes, Instant::now());
                health.observe_delivery(from_steam_id64, delivery_stream_id, delivery_sequence);
            });
            pending = Some(PendingHookDelivery {
                packet,
                receipt: delivery.receipt,
                summary,
                received_bytes,
            });
        }

        let Some(mut delivery) = pending.take() else {
            continue;
        };
        let started = Instant::now();
        let result = delivery
            .receipt
            .with_active(|| to_hook.try_send_recoverable(delivery.packet));
        match result {
            None => {
                delivery
                    .receipt
                    .complete_dropped(crate::client::lan::LanDataDropReason::GenerationClosed);
            }
            Some(Ok(())) => {
                observe_health(&health, |health| {
                    health.observe_hook_out_send_duration(started.elapsed());
                });
                send_event(
                    &event_tx,
                    RuntimeEvent::CounterDelta(network_in_counter(delivery.received_bytes)),
                );
                observer.observe_network_packet(&event_tx, &delivery.summary);
                delivery.receipt.complete_accepted();
            }
            Some(Err(hook_ipc::ClientIpcTrySendError::Full(packet))) => {
                delivery.packet = packet;
                pending = Some(delivery);
                tokio::select! {
                    () = cancellation.cancelled() => {
                        if let Some(delivery) = pending.take() {
                            delivery
                                .receipt
                                .complete_dropped(crate::client::lan::LanDataDropReason::SessionClosed);
                        }
                        return Ok(());
                    },
                    () = time::sleep(DIRECT_HOOK_RETRY_INTERVAL) => {}
                }
            }
            Some(Err(hook_ipc::ClientIpcTrySendError::Disconnected(packet))) => {
                drop(packet);
                delivery
                    .receipt
                    .complete_dropped(crate::client::lan::LanDataDropReason::HookDisconnected);
                return Err(io::Error::new(
                    io::ErrorKind::BrokenPipe,
                    "Native Hook outbound queue is disconnected",
                ));
            }
        }
    }
}

pub(super) async fn direct_monitor_task(
    monitor: crate::client::lan::LanDataPlaneMonitor,
    event_tx: RuntimeEventSender,
    cancellation: CancellationToken,
) -> io::Result<()> {
    loop {
        for transition in monitor.drain_transitions() {
            send_event(
                &event_tx,
                log_event(
                    transition_level(transition.kind),
                    transition_log(&transition),
                ),
            );
        }
        tokio::select! {
            () = cancellation.cancelled() => {
                for transition in monitor.drain_transitions() {
                    send_event(
                        &event_tx,
                        log_event(transition_level(transition.kind), transition_log(&transition)),
                    );
                }
                return Ok(());
            }
            () = monitor.changed() => {}
        }
    }
}

pub(super) async fn emit_direct_summary(
    event_tx: &RuntimeEventSender,
    monitor: &Option<crate::client::lan::LanDataPlaneMonitor>,
) {
    let Some(monitor) = monitor else {
        return;
    };
    let snapshot = monitor.snapshot();
    send_critical_event(
        event_tx,
        log_event(
            LogLevel::Info,
            format!(
                "Direct data summary peers={} send_queued={} send_succeeded={} send_dropped={} receive_queued={} hook_queue_accepted={} receive_dropped={} transitions_dropped={}",
                snapshot.peers.len(),
                snapshot.send.queued,
                snapshot.send.resolved_success,
                snapshot.send.dropped,
                snapshot.receive.queued,
                snapshot.receive.resolved_success,
                snapshot.receive.dropped,
                snapshot.transitions_dropped,
            ),
        ),
    )
    .await;
}

fn transition_level(kind: crate::client::lan::LanIncidentTransitionKind) -> LogLevel {
    match kind {
        crate::client::lan::LanIncidentTransitionKind::Started => LogLevel::Warn,
        crate::client::lan::LanIncidentTransitionKind::Recovered
        | crate::client::lan::LanIncidentTransitionKind::Closed => LogLevel::Info,
    }
}

fn transition_log(transition: &crate::client::lan::LanIncidentTransition) -> String {
    format!(
        "Direct data incident kind={:?} peer_slot={} epoch={} direction={:?} stage={:?} reason={:?} outage_ms={} packets_dropped={}",
        transition.kind,
        transition.peer_slot,
        transition.lifecycle_epoch,
        transition.direction,
        transition.stage,
        transition.reason,
        transition.duration.as_millis(),
        transition.dropped_packets,
    )
}
