use super::*;

pub(super) struct RelayTransportTaskContext {
    pub(super) event_tx: RuntimeEventSender,
    pub(super) cancellation: CancellationToken,
    pub(super) health: Option<SharedSessionHealth>,
    pub(super) health_snapshot_interval: Duration,
    pub(super) runtime_rtt_interval: Duration,
}

pub(super) async fn hook_in_task(
    mut hook_packets_rx: TokioReceiver<tractor_beam_hook_ipc::GamePacket>,
    steam_id64: String,
    outbound_tx: TokioSender<OutboundRelayPacket>,
    event_tx: RuntimeEventSender,
    cancellation: CancellationToken,
    health: Option<SharedSessionHealth>,
) -> io::Result<()> {
    loop {
        tokio::select! {
            () = cancellation.cancelled() => return Ok(()),
            Some(packet) = hook_packets_rx.recv() => {
                let size = packet.payload.len();
                observe_health(&health, |health| health.observe_hook_in_recv(size, Instant::now()));
                match encode_outbound_relay_packet(&steam_id64, packet) {
                    Ok(packet) => {
                        let accepted = outbound_tx.try_send(packet).is_ok();
                        observe_health(&health, |health| health.observe_outbound_enqueue(accepted));
                        if !accepted {
                            send_error(&event_tx, "Relay outbound queue is full; dropping hook packet");
                        }
                    }
                    Err(error) => send_error(&event_tx, format!("Bad hook packet: {error}")),
                }
            }
        }
    }
}

pub(super) async fn relay_transport_task(
    mut relay: RelayTransport,
    mut outbound_rx: TokioReceiver<OutboundRelayPacket>,
    inbound_tx: TokioSender<InboundGamePacket>,
    context: RelayTransportTaskContext,
) -> io::Result<()> {
    let mut observer = PacketObserver::default();
    let mut heartbeat = time::interval(HEARTBEAT_INTERVAL);
    heartbeat.set_missed_tick_behavior(MissedTickBehavior::Delay);
    let mut health_snapshot = time::interval(context.health_snapshot_interval);
    health_snapshot.set_missed_tick_behavior(MissedTickBehavior::Delay);
    let mut runtime_rtt = time::interval(context.runtime_rtt_interval);
    runtime_rtt.set_missed_tick_behavior(MissedTickBehavior::Delay);

    loop {
        tokio::select! {
            () = context.cancellation.cancelled() => {
                return Ok(());
            }
            Some(packet) = outbound_rx.recv() => {
                let started = Instant::now();
                relay.sender.send_data_datagram(packet.raw).await?;
                observe_health(&context.health, |health| {
                    health.observe_relay_send_duration(started.elapsed());
                });
                send_event(&context.event_tx, RuntimeEvent::CounterDelta(hook_counter(packet.sent_bytes)));
                observer.observe_hook_packet(&context.event_tx, &packet.summary);
            }
            raw = relay.receiver.recv_datagram() => {
                let raw = raw?;
                observe_health(&context.health, |health| {
                    health.observe_relay_recv(raw.len(), Instant::now());
                });
                match decode_inbound_relay_datagram(raw) {
                    Ok(Some(InboundRelayDatagram::Game(packet))) => {
                        observe_health(&context.health, |health| {
                            let peer = packet.game.from_steam_id64.parse::<u64>().unwrap_or_default();
                            health.observe_source_sequence(peer, packet.game.source_sequence);
                        });
                        let accepted = inbound_tx.try_send(packet).is_ok();
                        observe_health(&context.health, |health| health.observe_inbound_enqueue(accepted));
                        if !accepted {
                            send_error(&context.event_tx, "Hook inbound queue is full; dropping relay packet");
                        }
                    }
                    Ok(Some(InboundRelayDatagram::HealthPong { id })) => {
                        observe_health(&context.health, |health| health.observe_health_pong(id, Instant::now()));
                    }
                    Ok(Some(InboundRelayDatagram::RoomUpdate { peers })) => {
                        send_event(&context.event_tx, RuntimeEvent::RoomPeersUpdated(peers));
                    }
                    Ok(None) => {}
                    Err(error) => send_error(&context.event_tx, format!("Bad relay packet: {error}")),
                }
            }
            _ = heartbeat.tick() => {
                send_control(&mut relay.sender, MessageType::Heartbeat, &ControlMessage::Heartbeat).await?;
            }
            _ = health_snapshot.tick(), if context.health.is_some() => {
                emit_health_snapshot(&context.event_tx, &context.health);
            }
            _ = runtime_rtt.tick(), if context.health.is_some() => {
                if let Some(id) = next_health_ping(&context.health) {
                    send_control(&mut relay.sender, MessageType::Heartbeat, &ControlMessage::HealthPing { id }).await?;
                }
            }
        }
    }
}

pub(super) async fn hook_out_task(
    to_hook: hook_ipc::ClientIpcSender,
    mut inbound_rx: TokioReceiver<InboundGamePacket>,
    event_tx: RuntimeEventSender,
    cancellation: CancellationToken,
    health: Option<SharedSessionHealth>,
) -> io::Result<()> {
    let mut local_sequence = 1_u32;
    let mut observer = PacketObserver::default();
    loop {
        tokio::select! {
            () = cancellation.cancelled() => return Ok(()),
            Some(packet) = inbound_rx.recv() => {
                let (packet, summary, received_bytes) =
                    encode_inbound_hook_packet(packet, &mut local_sequence);
                let started = Instant::now();
                let accepted = to_hook.try_send(packet);
                observe_health(&health, |health| {
                    health.observe_hook_out_send_duration(started.elapsed());
                });
                if accepted {
                    send_event(&event_tx, RuntimeEvent::CounterDelta(relay_counter(received_bytes)));
                    observer.observe_relay_packet(&event_tx, &summary);
                } else {
                    send_error(&event_tx, "Native Hook outbound queue is full; dropping relay packet");
                }
            }
        }
    }
}

fn observe_health(health: &Option<SharedSessionHealth>, observe: impl FnOnce(&mut SessionHealth)) {
    let Some(health) = health else {
        return;
    };
    if let Ok(mut health) = health.lock() {
        observe(&mut health);
    }
}

fn next_health_ping(health: &Option<SharedSessionHealth>) -> Option<u64> {
    health
        .as_ref()
        .and_then(|health| health.lock().ok()?.next_health_ping(Instant::now()))
}

fn emit_health_snapshot(event_tx: &RuntimeEventSender, health: &Option<SharedSessionHealth>) {
    if let Some(snapshot) = current_health_snapshot(health) {
        send_event(
            event_tx,
            log_event(LogLevel::Info, snapshot.compact_log_line("Session health")),
        );
        send_event(
            event_tx,
            RuntimeEvent::SessionHealthSnapshot(Box::new(snapshot)),
        );
    }
}

pub(super) async fn emit_health_summary(
    event_tx: &RuntimeEventSender,
    health: &Option<SharedSessionHealth>,
) {
    if let Some(snapshot) = current_health_snapshot(health) {
        send_event(
            event_tx,
            log_event(
                LogLevel::Info,
                snapshot.compact_log_line("Session health summary"),
            ),
        );
        send_critical_event(
            event_tx,
            RuntimeEvent::SessionHealthSummary(Box::new(snapshot)),
        )
        .await;
    }
}

fn current_health_snapshot(health: &Option<SharedSessionHealth>) -> Option<SessionHealthSnapshot> {
    health
        .as_ref()
        .and_then(|health| Some(health.lock().ok()?.snapshot(Instant::now())))
}
