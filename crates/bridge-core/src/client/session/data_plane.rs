use super::*;
use backon::{BackoffBuilder as _, ExponentialBuilder};

use crate::client::Counters;
use crate::client::relay_transport::RecoveryKind;

const RECOVERY_DEADLINE: Duration = Duration::from_secs(120);
const RECOVERY_ATTEMPT_TIMEOUT: Duration = Duration::from_secs(5);

pub(super) struct RelayTransportTaskContext {
    pub(super) event_tx: RuntimeEventSender,
    pub(super) cancellation: CancellationToken,
    pub(super) health: Option<SharedSessionHealth>,
    pub(super) health_snapshot_interval: Duration,
    pub(super) runtime_rtt_interval: Duration,
    pub(super) initial_peers: Vec<PeerPresenceInfo>,
}

pub(super) async fn hook_in_task(
    mut hook_packets_rx: TokioReceiver<tractor_beam_hook_ipc::GamePacket>,
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
                match encode_outbound_relay_packet(packet) {
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
    let mut room_path_tick = time::interval(Duration::from_secs(1));
    room_path_tick.set_missed_tick_behavior(MissedTickBehavior::Delay);
    let local_steam_id64 = relay.local_steam_id64();
    let mut room_peers = context.initial_peers.clone();
    let mut room_path = RoomPathQuality::default();
    if relay.supports_room_path_probe() {
        room_path.sync_peers(&room_peers, local_steam_id64);
    }

    loop {
        tokio::select! {
            () = context.cancellation.cancelled() => {
                let _ = send_control(&mut relay.sender, &ClientControl::Stop).await;
                return Ok(());
            }
            Some(packet) = outbound_rx.recv() => {
                let started = Instant::now();
                let summary = packet.summary;
                let sent_bytes = packet.sent_bytes;
                if let Err(error) = relay.sender.send_data_datagram(packet).await {
                    reset_room_path(&context.event_tx, &mut room_path);
                    room_peers = recover_relay(&mut relay, &mut outbound_rx, &context, error).await?;
                    sync_room_path(&context.event_tx, &relay, &room_peers, &mut room_path);
                    continue;
                }
                observe_health(&context.health, |health| {
                    health.observe_relay_send_duration(started.elapsed());
                });
                send_event(&context.event_tx, RuntimeEvent::CounterDelta(hook_counter(sent_bytes)));
                observer.observe_hook_packet(&context.event_tx, &summary);
            }
            raw = relay.receiver.recv_datagram() => {
                let raw = match raw {
                    Ok(raw) => raw,
                    Err(error) => {
                        reset_room_path(&context.event_tx, &mut room_path);
                        room_peers = recover_relay(&mut relay, &mut outbound_rx, &context, error).await?;
                        sync_room_path(&context.event_tx, &relay, &room_peers, &mut room_path);
                        continue;
                    }
                };
                observe_health(&context.health, |health| {
                    health.observe_relay_recv(raw.len(), Instant::now());
                });
                match decode_inbound_relay_datagram(raw) {
                    Ok(Some(InboundRelayDatagram::Game(packet))) => {
                        observe_health(&context.health, |health| {
                            let peer = packet.game.from_steam_id64;
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
                    Ok(Some(InboundRelayDatagram::PeerPresence { peers })) => {
                        room_peers = peers;
                        send_event(&context.event_tx, RuntimeEvent::RoomPeersUpdated(room_peers.clone()));
                        sync_room_path(&context.event_tx, &relay, &room_peers, &mut room_path);
                    }
                    Ok(Some(InboundRelayDatagram::Probe(probe))) => {
                        match probe.phase {
                            ProbePhase::Request if probe.to_steam_id64 == local_steam_id64 => {
                                let target_supported = room_peers.iter().any(|peer| {
                                    peer.steam_id64 == probe.from_steam_id64
                                        && peer.presence == crate::protocol::v2::PeerPresence::Connected
                                        && peer.capabilities & crate::protocol::v2::CAP_ROOM_PATH_PROBE != 0
                                });
                                if target_supported {
                                    let _ = relay.sender.send_probe(
                                        probe.from_steam_id64,
                                        probe.probe_id,
                                        ProbePhase::Echo,
                                    ).await;
                                }
                            }
                            ProbePhase::Echo if probe.to_steam_id64 == local_steam_id64 => {
                                if room_path.record_echo(
                                    probe.from_steam_id64,
                                    probe.probe_id,
                                    Instant::now(),
                                ) {
                                    emit_room_path(&context.event_tx, &room_path);
                                }
                            }
                            _ => {}
                        }
                    }
                    Ok(None) => {}
                    Err(error) => send_error(&context.event_tx, format!("Bad relay packet: {error}")),
                }
            }
            _ = heartbeat.tick() => {
                if let Err(error) = send_control(&mut relay.sender, &ClientControl::ControlPing { id: 0 }).await {
                    recover_relay(&mut relay, &mut outbound_rx, &context, error).await?;
                }
            }
            _ = health_snapshot.tick(), if context.health.is_some() => {
                emit_health_snapshot(&context.event_tx, &context.health);
            }
            _ = runtime_rtt.tick(), if context.health.is_some() => {
                if let Some(id) = next_health_ping(&context.health)
                    && let Err(error) = send_control(&mut relay.sender, &ClientControl::ControlPing { id }).await
                {
                    recover_relay(&mut relay, &mut outbound_rx, &context, error).await?;
                }
            }
            _ = room_path_tick.tick(), if relay.supports_room_path_probe() => {
                let now = Instant::now();
                room_path.expire(now);
                let targets = room_path.targets().collect::<Vec<_>>();
                for target in targets {
                    let probe_id = relay.sender.next_probe_id();
                    if relay.sender.send_probe(target, probe_id, ProbePhase::Request).await.is_ok() {
                        room_path.record_sent(target, probe_id, now);
                    }
                }
                emit_room_path(&context.event_tx, &room_path);
            }
        }
    }
}

async fn recover_relay(
    relay: &mut RelayTransport,
    outbound_rx: &mut TokioReceiver<OutboundRelayPacket>,
    context: &RelayTransportTaskContext,
    initial_error: io::Error,
) -> io::Result<Vec<PeerPresenceInfo>> {
    let started = Instant::now();
    let mut last_error = initial_error.to_string();
    let mut attempt = 0_u32;
    let mut dropped = 0_u64;
    let mut backoff = ExponentialBuilder::default()
        .with_min_delay(Duration::from_millis(250))
        .with_max_delay(Duration::from_secs(2))
        .with_jitter()
        .build();

    loop {
        attempt = attempt.saturating_add(1);
        let elapsed = started.elapsed();
        send_event(
            &context.event_tx,
            RuntimeEvent::RelayLinkChanged(crate::client::RelayLinkState::Reconnecting {
                attempt,
                elapsed_ms: elapsed.as_millis(),
                last_error: last_error.clone(),
                data_continues: false,
            }),
        );
        send_event(
            &context.event_tx,
            log_event(
                LogLevel::Warn,
                format!(
                    "relay_reconnect_attempt attempt={attempt} elapsed_ms={} profile_reconnect_drops={dropped} failure={last_error}",
                    elapsed.as_millis()
                ),
            ),
        );

        let result = tokio::select! {
            () = context.cancellation.cancelled() => {
                return Err(io::Error::new(io::ErrorKind::Interrupted, "Relay recovery cancelled"));
            }
            result = time::timeout(RECOVERY_ATTEMPT_TIMEOUT, relay.reconnect()) => {
                match result {
                    Ok(result) => result,
                    Err(_) => Err(io::Error::new(io::ErrorKind::TimedOut, "Relay recovery attempt timed out")),
                }
            }
        };
        match result {
            Ok(recovery) => {
                let (peers, full_join) = match recovery {
                    RecoveryKind::Resumed { peers } => (peers, false),
                    RecoveryKind::FullJoin { peers } => (peers, true),
                };
                let outage_ms = started.elapsed().as_millis();
                send_event(
                    &context.event_tx,
                    RuntimeEvent::RoomPeersUpdated(peers.clone()),
                );
                send_event(
                    &context.event_tx,
                    RuntimeEvent::RelayLinkChanged(crate::client::RelayLinkState::Recovered {
                        attempts: attempt,
                        outage_ms,
                        full_join,
                    }),
                );
                send_event(
                    &context.event_tx,
                    log_event(
                        LogLevel::Info,
                        format!(
                            "relay_reconnect_succeeded attempts={attempt} outage_ms={outage_ms} recovery={} packets_dropped={dropped}",
                            if full_join { "full_join" } else { "resume" }
                        ),
                    ),
                );
                return Ok(peers);
            }
            Err(error) => last_error = error.to_string(),
        }

        if started.elapsed() >= RECOVERY_DEADLINE {
            let elapsed_ms = started.elapsed().as_millis();
            send_event(
                &context.event_tx,
                RuntimeEvent::RelayLinkChanged(crate::client::RelayLinkState::RecoveryExhausted {
                    attempts: attempt,
                    elapsed_ms,
                    reason: last_error.clone(),
                }),
            );
            send_event(
                &context.event_tx,
                log_event(
                    LogLevel::Error,
                    format!(
                        "relay_reconnect_exhausted attempts={attempt} elapsed_ms={elapsed_ms} packets_dropped={dropped} failure={last_error}"
                    ),
                ),
            );
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                format!("Relay recovery exhausted after {elapsed_ms} ms: {last_error}"),
            ));
        }

        let delay = backoff.next().unwrap_or(Duration::from_secs(2));
        let remaining = RECOVERY_DEADLINE.saturating_sub(started.elapsed());
        let sleep = time::sleep(delay.min(remaining));
        tokio::pin!(sleep);
        loop {
            tokio::select! {
                () = context.cancellation.cancelled() => {
                    return Err(io::Error::new(io::ErrorKind::Interrupted, "Relay recovery cancelled"));
                }
                _ = &mut sleep => break,
                packet = outbound_rx.recv() => {
                    if packet.is_some() {
                        dropped = dropped.saturating_add(1);
                        send_event(
                            &context.event_tx,
                            RuntimeEvent::CounterDelta(Counters {
                                reconnect_dropped_packets: 1,
                                ..Counters::default()
                            }),
                        );
                    }
                }
            }
        }
    }
}

fn sync_room_path(
    event_tx: &RuntimeEventSender,
    relay: &RelayTransport,
    peers: &[PeerPresenceInfo],
    room_path: &mut RoomPathQuality,
) {
    if relay.supports_room_path_probe() {
        room_path.sync_peers(peers, relay.local_steam_id64());
    } else {
        room_path.clear();
    }
    emit_room_path(event_tx, room_path);
}

fn reset_room_path(event_tx: &RuntimeEventSender, room_path: &mut RoomPathQuality) {
    room_path.clear();
    emit_room_path(event_tx, room_path);
}

fn emit_room_path(event_tx: &RuntimeEventSender, room_path: &RoomPathQuality) {
    send_event(
        event_tx,
        RuntimeEvent::RoomPathQualityUpdated(room_path.snapshots(Instant::now())),
    );
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
