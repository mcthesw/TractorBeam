use super::*;

pub(super) async fn supervise_session(
    config: SessionConfig,
    native_hook: Option<SessionNativeHook>,
    control: SessionControl,
    cancellation: CancellationToken,
    std_event_tx: mpsc::Sender<RuntimeEvent>,
    startup_tx: SyncSender<io::Result<()>>,
    relay_data_plane: Option<RelayRoomDataPlane>,
) {
    let SessionControl {
        ipc: ipc_control_rx,
        gameplay: mut command_rx,
    } = control;
    let (event_tx, event_rx) = tokio_mpsc::channel(EVENT_QUEUE_CAPACITY);
    let event_forwarder = tokio::spawn(forward_events(event_rx, std_event_tx));

    match start_runtime_tasks(
        &config,
        native_hook,
        ipc_control_rx,
        &cancellation,
        &event_tx,
        relay_data_plane,
    )
    .await
    {
        Ok(mut runtime_tasks) => {
            send_startup(&startup_tx, Ok(()));
            send_event(
                &event_tx,
                log_event(LogLevel::Info, "Session runtime is running"),
            );
            if config.mode != SessionMode::Official
                && let SessionRouteConfig::ExternalRelay(route) = &config.route
            {
                send_event(
                    &event_tx,
                    log_event(
                        LogLevel::Debug,
                        format!(
                            "Bridge local IPC ready: version={}.{} relay={} transport={} packet_queue={PACKET_QUEUE_CAPACITY}",
                            tractor_beam_hook_ipc::PROTOCOL_MAJOR,
                            tractor_beam_hook_ipc::PROTOCOL_MINOR,
                            route.relay,
                            route.transport
                        ),
                    ),
                );
            }

            loop {
                let route_active = runtime_tasks
                    .route
                    .as_ref()
                    .is_some_and(|route| !route.tasks.is_empty());
                let commands_active = command_rx.is_some();
                tokio::select! {
                    () = cancellation.cancelled() => break,
                    command = async {
                        command_rx
                            .as_mut()
                            .expect("active gameplay command receiver exists")
                            .recv()
                            .await
                    }, if commands_active => {
                        let Some(command) = command else {
                            command_rx = None;
                            continue;
                        };
                        handle_session_command(
                            command,
                            &mut runtime_tasks,
                            &cancellation,
                            &event_tx,
                        )
                        .await;
                    }
                    result = async {
                        runtime_tasks
                            .route
                            .as_mut()
                            .expect("active route exists")
                            .tasks
                            .join_next()
                            .await
                    }, if route_active => {
                        let route_cancellation = runtime_tasks
                            .route
                            .as_ref()
                            .expect("active route exists")
                            .cancellation
                            .clone();
                        let message = task_exit_message(
                            "Bridge gameplay task",
                            &route_cancellation,
                            result,
                        );
                        if let Some(route) = runtime_tasks.route.take() {
                            stop_route_tasks(route, &event_tx).await;
                        }
                        if let Some(message) = message {
                            send_critical_event(
                                &event_tx,
                                RuntimeEvent::SessionEnded(SessionStopReason::RuntimeEnded {
                                    message: message.clone(),
                                }),
                            )
                            .await;
                            send_event(&event_tx, log_event(LogLevel::Warn, message));
                        }
                        send_critical_event(&event_tx, RuntimeEvent::GameplayStopped).await;
                    }
                    result = runtime_tasks.support.join_next(), if !runtime_tasks.support.is_empty() => {
                        if let Some(message) = task_exit_message(
                            "Bridge lifecycle task",
                            &cancellation,
                            result,
                        ) {
                            send_critical_event(
                                &event_tx,
                                RuntimeEvent::SessionEnded(SessionStopReason::RuntimeEnded {
                                    message: message.clone(),
                                }),
                            )
                            .await;
                            send_event(&event_tx, log_event(LogLevel::Warn, message));
                        }
                        break;
                    }
                }
            }
            cancellation.cancel();
            if let Some(route) = runtime_tasks.route.take() {
                stop_route_tasks(route, &event_tx).await;
            }
            shutdown_tasks(runtime_tasks.support, &event_tx).await;
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
    relay_data_plane: Option<RelayRoomDataPlane>,
) -> io::Result<RuntimeTasks> {
    tokio::select! {
        result = start_runtime_tasks_inner(
            config,
            native_hook,
            ipc_control_rx,
            cancellation,
            event_tx,
            relay_data_plane,
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
    relay_data_plane: Option<RelayRoomDataPlane>,
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
            Vec::new(),
            event_tx.clone(),
            cancellation.clone(),
            None,
        ));
        return Ok(RuntimeTasks {
            route: None,
            support,
            hook_outbound: None,
            to_hook: None,
        });
    }

    let native_hook = native_hook.expect("Native Hook presence was validated above");
    let preexisting_processes = native_hook.preexisting_processes;
    let ready_deadline = native_hook.ipc.ready_deadline();
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
    let mut support = JoinSet::new();
    support.spawn(ipc_worker);
    support.spawn(process_lifecycle::run(
        Some(native_hook.paths),
        preexisting_processes,
        event_tx.clone(),
        cancellation.clone(),
        Some(ready_deadline),
    ));
    let hook_outbound = HookOutboundSlot::new();
    support.spawn(hook_dispatch_task(
        hook_packets_rx,
        hook_outbound.clone(),
        event_tx.clone(),
        cancellation.clone(),
    ));
    let route = start_route_tasks(
        config,
        &hook_outbound,
        to_hook.clone(),
        cancellation,
        event_tx,
        relay_data_plane,
    )
    .await?;
    Ok(RuntimeTasks {
        route: Some(route),
        support,
        hook_outbound: Some(hook_outbound),
        to_hook: Some(to_hook),
    })
}

async fn start_route_tasks(
    config: &SessionConfig,
    hook_outbound: &HookOutboundSlot,
    to_hook: hook_ipc::ClientIpcSender,
    cancellation: &CancellationToken,
    event_tx: &RuntimeEventSender,
    mut relay_data_plane: Option<RelayRoomDataPlane>,
) -> io::Result<RouteTasks> {
    let route_cancellation = cancellation.child_token();
    let mut hook_outbound = hook_outbound.attach()?;
    let hook_packets_rx = hook_outbound.receiver.take().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::AlreadyExists,
            "Native Hook gameplay packets are already attached",
        )
    })?;
    let health = config.session_health.enabled.then(|| {
        Arc::new(Mutex::new(SessionHealth::new(
            config.session_health.runtime_rtt_enabled
                && relay_data_plane.is_none()
                && matches!(&config.route, SessionRouteConfig::ExternalRelay(_)),
            Duration::from_secs(config.session_health.runtime_rtt_timeout_seconds),
            Instant::now(),
        )))
    });
    let mut tasks = JoinSet::new();
    let (direct_monitor, lan_data_plane) = match &config.route {
        SessionRouteConfig::ExternalRelay(relay_route) => {
            if let Some(data_plane) = relay_data_plane.as_mut() {
                let inbound_rx = data_plane.inbound_rx.take().ok_or_else(|| {
                    io::Error::new(
                        io::ErrorKind::AlreadyExists,
                        "Relay room gameplay data plane is already attached",
                    )
                })?;
                tasks.spawn(hook_in_task(
                    hook_packets_rx,
                    data_plane.outbound_tx.clone(),
                    event_tx.clone(),
                    route_cancellation.clone(),
                    health.clone(),
                ));
                tasks.spawn(hook_out_task(
                    to_hook,
                    inbound_rx,
                    event_tx.clone(),
                    route_cancellation.clone(),
                    health.clone(),
                ));
            } else {
                let (outbound_tx, outbound_rx) = tokio_mpsc::channel(PACKET_QUEUE_CAPACITY);
                tasks.spawn(hook_in_task(
                    hook_packets_rx,
                    outbound_tx,
                    event_tx.clone(),
                    route_cancellation.clone(),
                    health.clone(),
                ));
                let (relay, peers) = RelayTransport::connect_session(
                    relay_route,
                    &config.steam_id64,
                    &config.display_name,
                )
                .await?;
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
                let (inbound_tx, inbound_rx) = tokio_mpsc::channel(PACKET_QUEUE_CAPACITY);
                tasks.spawn(relay_transport_task(
                    relay,
                    outbound_rx,
                    RelayInboundTarget::Fixed(inbound_tx),
                    RelayTransportTaskContext {
                        event_tx: event_tx.clone(),
                        cancellation: route_cancellation.clone(),
                        health: health.clone(),
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
                    route_cancellation.clone(),
                    health.clone(),
                ));
            }
            (None, None)
        }
        SessionRouteConfig::LanDirect(lan_route) => {
            let room = lan_route.room.clone().ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidInput, "Direct LAN room is not active")
            })?;
            if !room.uses_credential(lan_route.session_credential) {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "Direct LAN room credential does not match the session",
                ));
            }
            let send_observer = Arc::new(DirectSendObserver::new(event_tx.clone(), health.clone()));
            let (inbound, monitor, attachment) =
                room.take_data_plane(send_observer).ok_or_else(|| {
                    io::Error::new(
                        io::ErrorKind::AlreadyExists,
                        "Direct LAN room data plane is already attached",
                    )
                })?;
            tasks.spawn(direct_hook_in_task(
                room,
                hook_packets_rx,
                route_cancellation.clone(),
                health.clone(),
            ));
            tasks.spawn(direct_hook_out_task(
                to_hook,
                inbound,
                event_tx.clone(),
                route_cancellation.clone(),
                health.clone(),
            ));
            tasks.spawn(direct_monitor_task(
                monitor.clone(),
                event_tx.clone(),
                route_cancellation.clone(),
            ));
            send_event(
                event_tx,
                log_event(LogLevel::Info, "Direct LAN route is attached"),
            );
            (Some(monitor), Some(attachment))
        }
    };
    if health.is_some() {
        tasks.spawn(health_snapshot_task(
            event_tx.clone(),
            route_cancellation.clone(),
            health.clone(),
            direct_monitor.clone(),
            Duration::from_secs(config.session_health.snapshot_interval_seconds),
        ));
    }
    Ok(RouteTasks {
        tasks,
        cancellation: route_cancellation,
        health,
        direct_monitor,
        _hook_outbound: Some(hook_outbound),
        _relay_data_plane: relay_data_plane,
        _lan_data_plane: lan_data_plane,
    })
}

async fn handle_session_command(
    command: SessionCommand,
    runtime: &mut RuntimeTasks,
    cancellation: &CancellationToken,
    event_tx: &RuntimeEventSender,
) {
    match command {
        SessionCommand::StartGameplay {
            config,
            relay_data_plane,
            reply,
        } => {
            let result = if runtime.route.is_some() {
                Err(io::Error::new(
                    io::ErrorKind::AlreadyExists,
                    "Gameplay is already attached",
                ))
            } else {
                match (&runtime.hook_outbound, &runtime.to_hook) {
                    (Some(hook_outbound), Some(to_hook)) => start_route_tasks(
                        &config,
                        hook_outbound,
                        to_hook.clone(),
                        cancellation,
                        event_tx,
                        relay_data_plane,
                    )
                    .await
                    .map(|route| runtime.route = Some(route)),
                    _ => Err(io::Error::new(
                        io::ErrorKind::Unsupported,
                        "Native Hook runtime is unavailable",
                    )),
                }
            };
            if result.is_ok() {
                send_event(
                    event_tx,
                    log_event(
                        LogLevel::Info,
                        "Gameplay route attached without restarting Isaac",
                    ),
                );
            }
            let _ = reply.send(result);
        }
        SessionCommand::StopGameplay { reply } => {
            if let Some(route) = runtime.route.take() {
                stop_route_tasks(route, event_tx).await;
            }
            send_event(
                event_tx,
                log_event(
                    LogLevel::Info,
                    "Gameplay route detached; Native Hook remains ready",
                ),
            );
            let _ = reply.send(Ok(()));
        }
    }
}

pub(super) async fn stop_route_tasks(route: RouteTasks, event_tx: &RuntimeEventSender) {
    route.cancellation.cancel();
    shutdown_tasks(route.tasks, event_tx).await;
    emit_direct_summary(event_tx, &route.direct_monitor).await;
    emit_health_summary(event_tx, &route.health, &route.direct_monitor).await;
}

#[cfg(test)]
pub(super) fn test_native_hook_paths() -> tractor_beam_isaac_injector::NativeHookPaths {
    tractor_beam_isaac_injector::NativeHookPaths {
        injector: PathBuf::from("tractor-beam-isaac-injector.exe"),
        hook: PathBuf::from("tractor_beam_native_hook.dll"),
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

pub(super) async fn forward_events(
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
