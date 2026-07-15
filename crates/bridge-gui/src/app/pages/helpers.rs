use super::*;

pub(super) fn room_path_quality_cells(
    ui: &mut egui::Ui,
    is_self: bool,
    quality: Option<&RoomPathQualitySnapshot>,
) {
    if is_self {
        for _ in 0..4 {
            ui.monospace("—");
        }
        return;
    }
    let Some(quality) = quality else {
        ui.monospace("—");
        ui.monospace("—");
        ui.monospace("—");
        ui.label(t!("room.path.unavailable"));
        return;
    };
    if quality.state == RoomPathQualityState::Measuring {
        ui.monospace("—");
        ui.monospace("—");
        ui.monospace("—");
        ui.label(t!("room.path.measuring"));
        return;
    }
    ui.monospace(display_room_path_duration(quality.median_rtt));
    ui.monospace(display_room_path_duration(quality.jitter));
    ui.monospace(quality.loss_basis_points.map_or_else(
        || "—".to_owned(),
        |value| format!("{:.2}%", f32::from(value) / 100.0),
    ));
    match quality.state {
        RoomPathQualityState::Current => ui.label(quality.freshness.map_or_else(
            || t!("room.path.current").into_owned(),
            |age| format!("{}s", age.as_secs()),
        )),
        RoomPathQualityState::Stale => ui.label(t!("room.path.stale")),
        RoomPathQualityState::Unavailable | RoomPathQualityState::Measuring => {
            ui.label(t!("room.path.unavailable"))
        }
    };
}

pub(super) fn display_room_path_duration(value: Option<std::time::Duration>) -> String {
    value.map_or_else(
        || "—".to_owned(),
        |value| format!("{} ms", value.as_millis()),
    )
}

pub(super) fn hook_phase_label(phase: HookStartupPhase) -> (egui::Color32, Cow<'static, str>) {
    match phase {
        HookStartupPhase::NotStarted => (egui::Color32::GRAY, t!("hook.not_started")),
        HookStartupPhase::Configured => (
            egui::Color32::from_rgb(100, 149, 237),
            t!("hook.configured"),
        ),
        HookStartupPhase::WaitingForIsaac => (
            egui::Color32::from_rgb(255, 200, 0),
            t!("hook.waiting_isaac"),
        ),
        HookStartupPhase::Injecting => (egui::Color32::from_rgb(255, 200, 0), t!("hook.injecting")),
        HookStartupPhase::WaitingForHookEndpoint => (
            egui::Color32::from_rgb(255, 200, 0),
            t!("hook.waiting_endpoint"),
        ),
        HookStartupPhase::EndpointReady => (
            egui::Color32::from_rgb(100, 200, 100),
            t!("hook.endpoint_ready"),
        ),
        HookStartupPhase::Ready => (egui::Color32::from_rgb(100, 200, 100), t!("hook.ready")),
        HookStartupPhase::Failed => (egui::Color32::from_rgb(220, 80, 80), t!("hook.failed")),
        HookStartupPhase::Cancelled => (egui::Color32::GRAY, t!("hook.cancelled")),
    }
}

pub(super) fn input_delay_controls_enabled(state: &RuntimeState) -> bool {
    state.status == SessionStatus::Running
        && matches!(
            state.active_session_mode,
            Some(SessionMode::Fallback | SessionMode::Pure)
        )
        && state.hook_ipc.connection == HookIpcConnectionState::Connected
}

pub(super) fn readiness_probe_table(ui: &mut egui::Ui, report: &ReadinessProbeReport) {
    let horizontal_spacing = 12.0;
    let columns = 5.0;
    let col_width =
        ((ui.available_width() - horizontal_spacing * (columns - 1.0)) / columns).max(72.0);
    egui::Grid::new("readiness_probe_table")
        .num_columns(5)
        .min_col_width(col_width)
        .striped(true)
        .spacing([horizontal_spacing, 4.0])
        .show(ui, |ui| {
            table_header(ui, t!("transport"));
            table_header(ui, t!("size"));
            table_header(ui, t!("lost"));
            table_header(ui, t!("latency"));
            table_header(ui, t!("jitter"));
            ui.end_row();

            for case in &report.cases {
                ui.label(connection_profile_label(case.connection_profile));
                ui.label(format!("{} B", case.payload_bytes));
                ui.label(lost_summary(case));
                ui.add(egui::Label::new(latency_summary(case)).wrap());
                ui.label(display_latency(case.jitter_ms));
                ui.end_row();
            }
        });
    let failed_cases = report
        .cases
        .iter()
        .filter_map(|case| case.failure_reason.as_ref().map(|reason| (case, reason)));
    for (index, (case, reason)) in failed_cases.enumerate() {
        if index == 0 {
            ui.add_space(4.0);
            ui.label(t!("details"));
        }
        wrapped_colored_label(
            ui,
            ui.visuals().error_fg_color,
            &format!(
                "{} {} B: {reason}",
                connection_profile_label(case.connection_profile),
                case.payload_bytes
            ),
        );
    }
    if report.cases.is_empty() {
        ui.add_space(4.0);
        ui.label(t!("probe.no_data"));
    }
}

pub(super) fn hook_probe_table(ui: &mut egui::Ui, report: &HookReceiveProbeReport) {
    egui::Grid::new("hook_probe_table")
        .num_columns(5)
        .striped(true)
        .spacing([12.0, 4.0])
        .show(ui, |ui| {
            table_header(ui, t!("hook.connection"));
            table_header(ui, t!("hook.protocol"));
            table_header(ui, t!("hook.reconnects"));
            table_header(ui, t!("hook.dropped"));
            table_header(ui, t!("hook.malformed"));
            ui.end_row();

            ui.label(&report.connection);
            ui.label(match (report.protocol_major, report.protocol_minor) {
                (Some(major), Some(minor)) => format!("{major}.{minor}"),
                _ => "-".to_owned(),
            });
            ui.label(report.reconnects.to_string());
            ui.label(format!(
                "{} / {}",
                report.hook_data_dropped, report.client_data_dropped
            ));
            ui.label(report.malformed_frames.to_string());
            ui.end_row();
        });
    if let Some(error) = &report.last_error {
        ui.add_space(4.0);
        wrapped_colored_label(ui, ui.visuals().error_fg_color, error);
    }
}

pub(super) fn session_health_summary(ui: &mut egui::Ui, state: &RuntimeState) {
    ui.horizontal(|ui| {
        ui.heading(t!("session_quality"));
        help_icon(ui, t!("help.session_quality"));
    });
    ui.add_space(6.0);
    let Some(snapshot) = &state.latest_session_health else {
        ui.label(t!("session.not_started"));
        return;
    };
    let quality = state.smoothness.level;
    let quality_color = match quality {
        SessionQuality::Good => ui.visuals().strong_text_color(),
        SessionQuality::Watch | SessionQuality::Poor => ui.visuals().error_fg_color,
        SessionQuality::Unavailable => ui.visuals().weak_text_color(),
    };
    ui.horizontal(|ui| {
        ui.colored_label(quality_color, "●");
        ui.label(quality_label(quality));
    });
    ui.label(smoothness_summary(quality));
    ui.add_space(4.0);
    egui::Grid::new("session_health_summary")
        .num_columns(2)
        .spacing([24.0, 4.0])
        .show(ui, |ui| {
            ui.label(t!("health.runtime_rtt"));
            ui.monospace(display_latency_ms(snapshot.runtime_rtt.latency.p95_ms));
            ui.end_row();

            ui.label(t!("health.queue_drops"));
            ui.monospace(snapshot.queues.total_dropped().to_string());
            ui.end_row();

            ui.label(t!("health.network_drops"));
            ui.monospace(snapshot.network_send_dropped.to_string());
            ui.end_row();

            ui.label(t!("health.sequence_gaps"));
            ui.monospace(snapshot.source_sequence.gaps.to_string());
            ui.end_row();

            ui.label(t!("health.packet_gaps"));
            ui.monospace(display_latency_ms(snapshot.network_recv.gap.p95_ms));
            ui.end_row();
        });
}

pub(super) fn table_header(ui: &mut egui::Ui, value: Cow<'static, str>) {
    ui.label(egui::RichText::new(value).strong());
}

pub(super) fn wrapped_colored_label(ui: &mut egui::Ui, color: egui::Color32, value: &str) {
    ui.add(egui::Label::new(egui::RichText::new(value).color(color)).wrap());
}

pub(super) fn latency_summary(report: &ReadinessProbeCaseReport) -> String {
    format!(
        "median={} ms p95={} ms",
        display_latency(report.median_latency_ms),
        display_latency(report.p95_latency_ms)
    )
}

pub(super) fn lost_summary(report: &ReadinessProbeCaseReport) -> String {
    if report.packets_sent == 0 {
        "-".to_owned()
    } else {
        format!("{}/{}", report.missing_packets, report.packets_sent)
    }
}

pub(super) fn display_latency(value: Option<u128>) -> String {
    value.map_or_else(
        || "-".to_owned(),
        |value| {
            if value == 0 {
                "<1".to_owned()
            } else {
                value.to_string()
            }
        },
    )
}

pub(super) fn display_latency_ms(value: Option<u64>) -> String {
    value.map_or_else(
        || "-".to_owned(),
        |value| {
            if value == 0 {
                "<1 ms".to_owned()
            } else {
                format!("{value} ms")
            }
        },
    )
}

pub(super) fn unix_seconds() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}
