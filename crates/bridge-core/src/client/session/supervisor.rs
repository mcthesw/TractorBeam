use super::*;

pub(super) async fn supervise_session(
    config: SessionConfig,
    native_hook: Option<SessionNativeHook>,
    ipc_control_rx: Option<Receiver<InputDelayCall>>,
    cancellation: CancellationToken,
    std_event_tx: mpsc::Sender<RuntimeEvent>,
    startup_tx: SyncSender<io::Result<()>>,
) {
    let (event_tx, event_rx) = tokio_mpsc::channel(EVENT_QUEUE_CAPACITY);
    let event_forwarder = tokio::spawn(forward_events(event_rx, std_event_tx));

    match start_runtime_tasks(
        &config,
        native_hook,
        ipc_control_rx,
        &cancellation,
        &event_tx,
    )
    .await
    {
        Ok(mut runtime_tasks) => {
            send_startup(&startup_tx, Ok(()));
            send_event(
                &event_tx,
                log_event(LogLevel::Info, "Session runtime is running"),
            );
            if config.mode != SessionMode::Official {
                send_event(
                    &event_tx,
                    log_event(
                        LogLevel::Debug,
                        format!(
                            "Bridge local IPC ready: version={}.{} relay={} transport={} packet_queue={PACKET_QUEUE_CAPACITY}",
                            tractor_beam_hook_ipc::PROTOCOL_MAJOR,
                            tractor_beam_hook_ipc::PROTOCOL_MINOR,
                            config.relay,
                            config.transport
                        ),
                    ),
                );
            }

            let stop_reason = wait_for_session_end(
                &cancellation,
                &mut runtime_tasks.essential,
                &mut runtime_tasks.support,
            )
            .await;
            cancellation.cancel();
            if let Some(message) = stop_reason {
                send_critical_event(
                    &event_tx,
                    RuntimeEvent::SessionEnded(SessionStopReason::RuntimeEnded {
                        message: message.clone(),
                    }),
                )
                .await;
                send_event(&event_tx, log_event(LogLevel::Warn, message));
            }
            shutdown_tasks(runtime_tasks.essential, &event_tx).await;
            shutdown_tasks(runtime_tasks.support, &event_tx).await;
            emit_health_summary(&event_tx, &runtime_tasks.health).await;
        }
        Err(error) => {
            let kind = error.kind();
            let message = error.to_string();
            send_startup(&startup_tx, Err(io::Error::new(kind, message.clone())));
            send_event(
                &event_tx,
                log_event(LogLevel::Error, format!("Bridge runtime failed: {message}")),
            );
            send_event(
                &event_tx,
                RuntimeEvent::HookStartup(Box::new(HookStartupState {
                    phase: HookStartupPhase::Failed,
                    message: Some(format!("Bridge runtime failed: {message}")),
                    updated_at: unix_seconds(),
                    ..HookStartupState::default()
                })),
            );
            send_critical_event(
                &event_tx,
                RuntimeEvent::SessionEnded(SessionStopReason::RuntimeEnded {
                    message: message.clone(),
                }),
            )
            .await;
            send_event(&event_tx, RuntimeEvent::CounterDelta(error_counter()));
        }
    }

    send_critical_event(&event_tx, RuntimeEvent::Stopped).await;
    drop(event_tx);
    let _ = event_forwarder.await;
}

async fn start_runtime_tasks(
    config: &SessionConfig,
    native_hook: Option<SessionNativeHook>,
    ipc_control_rx: Option<Receiver<InputDelayCall>>,
    cancellation: &CancellationToken,
    event_tx: &RuntimeEventSender,
) -> io::Result<RuntimeTasks> {
    tokio::select! {
        result = start_runtime_tasks_inner(
            config,
            native_hook,
            ipc_control_rx,
            cancellation,
            event_tx,
        ) => result,
        () = cancellation.cancelled() => Err(io::Error::new(
            io::ErrorKind::Interrupted,
            "bridge runtime startup cancelled",
        )),
    }
}

pub(super) async fn start_runtime_tasks_inner(
    config: &SessionConfig,
    native_hook: Option<SessionNativeHook>,
    ipc_control_rx: Option<Receiver<InputDelayCall>>,
    cancellation: &CancellationToken,
    event_tx: &RuntimeEventSender,
) -> io::Result<RuntimeTasks> {
    if config.mode != SessionMode::Official && native_hook.is_none() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Native Hook paths are required outside Official mode",
        ));
    }

    if config.mode == SessionMode::Official {
        let mut support = JoinSet::new();
        support.spawn(process_lifecycle::run(
            None,
            event_tx.clone(),
            cancellation.clone(),
        ));
        return Ok(RuntimeTasks {
            essential: JoinSet::new(),
            support,
            health: None,
        });
    }

    let native_hook = native_hook.expect("Native Hook presence was validated above");
    let ipc_control_rx = ipc_control_rx.ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "Native Hook local IPC control channel is required outside Official mode",
        )
    })?;
    let (hook_packets_rx, to_hook, ipc_worker) = hook_ipc::start(
        native_hook.ipc,
        ipc_control_rx,
        event_tx.clone(),
        cancellation.clone(),
    )?;
    let mut tasks = JoinSet::new();
    tasks.spawn(ipc_worker);
    let mut support = JoinSet::new();
    support.spawn(process_lifecycle::run(
        Some(native_hook.paths),
        event_tx.clone(),
        cancellation.clone(),
    ));
    let (relay, peers) = RelayTransport::connect_session(config).await?;
    let peer_count = peers.len();
    send_event(event_tx, RuntimeEvent::RoomPeersUpdated(peers.clone()));
    send_event(
        event_tx,
        RuntimeEvent::RelayLinkChanged(RelayLinkState::Connected),
    );
    send_event(
        event_tx,
        log_event(
            LogLevel::Info,
            format!("Joined relay room with {peer_count} peer(s)"),
        ),
    );

    let (outbound_tx, outbound_rx) = tokio_mpsc::channel(PACKET_QUEUE_CAPACITY);
    let (inbound_tx, inbound_rx) = tokio_mpsc::channel(PACKET_QUEUE_CAPACITY);
    let health = config.session_health.enabled.then(|| {
        Arc::new(Mutex::new(SessionHealth::new(
            config.session_health.runtime_rtt_enabled,
            Duration::from_secs(config.session_health.runtime_rtt_timeout_seconds),
            Instant::now(),
        )))
    });
    tasks.spawn(hook_in_task(
        hook_packets_rx,
        outbound_tx,
        event_tx.clone(),
        cancellation.clone(),
        health.clone(),
    ));
    tasks.spawn(relay_transport_task(
        relay,
        outbound_rx,
        inbound_tx,
        RelayTransportTaskContext {
            event_tx: event_tx.clone(),
            cancellation: cancellation.clone(),
            health: health.clone(),
            health_snapshot_interval: Duration::from_secs(
                config.session_health.snapshot_interval_seconds,
            ),
            runtime_rtt_interval: Duration::from_secs(
                config.session_health.runtime_rtt_interval_seconds,
            ),
            initial_peers: peers,
        },
    ));
    tasks.spawn(hook_out_task(
        to_hook,
        inbound_rx,
        event_tx.clone(),
        cancellation.clone(),
        health.clone(),
    ));

    Ok(RuntimeTasks {
        essential: tasks,
        support,
        health,
    })
}

#[cfg(test)]
pub(super) fn test_native_hook_paths() -> tractor_beam_isaac_injector::NativeHookPaths {
    tractor_beam_isaac_injector::NativeHookPaths {
        injector: PathBuf::from("tractor-beam-isaac-injector.exe"),
        hook: PathBuf::from("tractor_beam_native_hook.dll"),
    }
}

async fn wait_for_session_end(
    cancellation: &CancellationToken,
    essential: &mut JoinSet<io::Result<()>>,
    support: &mut JoinSet<io::Result<()>>,
) -> Option<String> {
    tokio::select! {
        () = cancellation.cancelled() => None,
        result = essential.join_next(), if !essential.is_empty() => {
            task_exit_message("Bridge session task", cancellation, result)
        }
        result = support.join_next(), if !support.is_empty() => {
            task_exit_message("Bridge lifecycle task", cancellation, result)
        }
    }
}

fn task_exit_message(
    task_name: &str,
    cancellation: &CancellationToken,
    result: Option<Result<io::Result<()>, tokio::task::JoinError>>,
) -> Option<String> {
    if cancellation.is_cancelled() {
        return None;
    }
    match result {
        Some(Ok(Ok(()))) => Some(format!("{task_name} exited")),
        Some(Ok(Err(error))) => Some(format!("{task_name} failed: {error}")),
        Some(Err(error)) => Some(format!("{task_name} panicked: {error}")),
        None => Some(format!("{task_name}s exited")),
    }
}

pub(super) async fn shutdown_tasks(
    mut tasks: JoinSet<io::Result<()>>,
    event_tx: &RuntimeEventSender,
) {
    if time::timeout(SHUTDOWN_TIMEOUT, drain_tasks(&mut tasks))
        .await
        .is_ok()
    {
        return;
    }
    tasks.abort_all();
    send_event(
        event_tx,
        log_event(
            LogLevel::Warn,
            "Bridge session shutdown timed out; aborted remaining tasks".to_owned(),
        ),
    );
    while tasks.join_next().await.is_some() {}
}

async fn drain_tasks(tasks: &mut JoinSet<io::Result<()>>) {
    while tasks.join_next().await.is_some() {}
}

async fn forward_events(
    mut event_rx: tokio_mpsc::Receiver<RuntimeEvent>,
    std_event_tx: mpsc::Sender<RuntimeEvent>,
) {
    while let Some(event) = event_rx.recv().await {
        if std_event_tx.send(event).is_err() {
            break;
        }
    }
}

pub(super) fn send_startup(sender: &SyncSender<io::Result<()>>, result: io::Result<()>) {
    let _ = sender.send(result);
}
