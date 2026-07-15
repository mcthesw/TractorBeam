use super::*;

pub(super) async fn lan_route_task(
    room: Arc<super::super::LanControlPlane>,
    mut outbound_rx: TokioReceiver<OutboundGamePacket>,
    event_tx: RuntimeEventSender,
    cancellation: CancellationToken,
) -> io::Result<()> {
    let mut observer = PacketObserver::default();
    loop {
        tokio::select! {
            () = cancellation.cancelled() => {
                room.stop().await;
                return Ok(());
            }
            Some(packet) = outbound_rx.recv() => {
                let sent_bytes = u64::try_from(packet.payload.len()).unwrap_or(u64::MAX);
                let summary = PacketSummary {
                    peer: packet.to_steam_id64,
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
                        send_event(
                            &event_tx,
                            RuntimeEvent::CounterDelta(network_out_counter(sent_bytes)),
                        );
                        observer.observe_hook_packet(&event_tx, &summary);
                    }
                    Err(error) => {
                        send_error(&event_tx, format!("Direct LAN packet dropped: {error}"));
                    }
                }
            }
        }
    }
}
