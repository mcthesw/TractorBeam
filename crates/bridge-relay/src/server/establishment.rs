use std::{collections::HashMap, sync::Arc, time::Duration};

use tokio::{sync::Mutex, time};
use tracing::{Span, info_span};

use crate::{domain::PeerId, metrics::RelayMetrics};

const TRACE_DEADLINE: Duration = Duration::from_secs(15);

#[derive(Clone)]
pub(super) struct EstablishmentRegistry {
    pending: Arc<Mutex<HashMap<u64, EstablishmentAttempt>>>,
}

impl EstablishmentRegistry {
    pub(super) fn new() -> Self {
        Self {
            pending: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub(super) async fn wait_for_udp(&self, connection_id: u64, attempt: EstablishmentAttempt) {
        self.pending.lock().await.insert(connection_id, attempt);
        let registry = self.clone();
        tokio::spawn(async move {
            time::sleep(TRACE_DEADLINE).await;
            registry
                .finish(connection_id, "timeout", "udp_validation_timeout")
                .await;
        });
    }

    pub(super) async fn complete_udp(&self, connection_id: u64) {
        let Some(attempt) = self.pending.lock().await.remove(&connection_id) else {
            return;
        };
        attempt.finish("accepted", None);
    }

    pub(super) async fn udp_span(&self, connection_id: u64) -> Span {
        self.pending
            .lock()
            .await
            .get(&connection_id)
            .map_or_else(Span::none, |attempt| {
                attempt.milestone_span(Milestone::UdpValidate)
            })
    }

    pub(super) async fn disconnect(&self, connection_id: u64) {
        self.finish(connection_id, "disconnected", "control_disconnected")
            .await;
    }

    async fn finish(&self, connection_id: u64, outcome: &'static str, error: &'static str) {
        if let Some(attempt) = self.pending.lock().await.remove(&connection_id) {
            attempt.finish(outcome, Some(error));
        }
    }
}

pub(super) struct EstablishmentAttempt {
    span: Span,
    metrics: Arc<RelayMetrics>,
    started: std::time::Instant,
    operation: &'static str,
    profile: &'static str,
}

impl EstablishmentAttempt {
    pub(super) fn start(peer_id: PeerId, metrics: Arc<RelayMetrics>) -> Self {
        Self {
            span: info_span!(
                parent: None,
                "relay.session.establish",
                relay.attempt_id = %peer_id,
                session.operation = tracing::field::Empty,
                network.transport = tracing::field::Empty,
                outcome = tracing::field::Empty,
                error.type = tracing::field::Empty,
                otel.status_code = tracing::field::Empty,
            ),
            metrics,
            started: std::time::Instant::now(),
            operation: "unknown",
            profile: "unknown",
        }
    }

    pub(super) const fn deadline() -> Duration {
        TRACE_DEADLINE
    }

    pub(super) fn set_route(&mut self, operation: &'static str, profile: &'static str) {
        self.operation = operation;
        self.profile = profile;
        self.span.record("session.operation", operation);
        self.span.record("network.transport", profile);
    }

    pub(super) fn milestone_span(&self, milestone: Milestone) -> Span {
        milestone.span(&self.span)
    }

    pub(super) fn finish(self, outcome: &'static str, error: Option<&'static str>) {
        self.span.record("outcome", outcome);
        if let Some(error) = error {
            self.span.record("error.type", error);
            self.span.record("otel.status_code", "ERROR");
        } else {
            self.span.record("otel.status_code", "OK");
        }
        self.metrics.record_establishment_duration(
            self.operation,
            self.profile,
            outcome,
            self.started.elapsed().as_secs_f64(),
        );
    }
}

#[derive(Clone, Copy)]
pub(super) enum Milestone {
    Bootstrap,
    JoinBegin,
    JoinProof,
    Resume,
    UdpValidate,
}

impl Milestone {
    fn span(self, parent: &Span) -> Span {
        match self {
            Self::Bootstrap => milestone_span(parent, "relay.bootstrap"),
            Self::JoinBegin => milestone_span(parent, "relay.join.begin"),
            Self::JoinProof => milestone_span(parent, "relay.join.proof"),
            Self::Resume => milestone_span(parent, "relay.resume"),
            Self::UdpValidate => milestone_span(parent, "relay.udp.validate"),
        }
    }
}

fn milestone_span(parent: &Span, name: &'static str) -> Span {
    match name {
        "relay.bootstrap" => {
            info_span!(parent: parent, "relay.bootstrap", outcome = tracing::field::Empty, error.type = tracing::field::Empty, otel.status_code = tracing::field::Empty)
        }
        "relay.join.begin" => {
            info_span!(parent: parent, "relay.join.begin", outcome = tracing::field::Empty, error.type = tracing::field::Empty, otel.status_code = tracing::field::Empty)
        }
        "relay.join.proof" => {
            info_span!(parent: parent, "relay.join.proof", outcome = tracing::field::Empty, error.type = tracing::field::Empty, otel.status_code = tracing::field::Empty)
        }
        "relay.resume" => {
            info_span!(parent: parent, "relay.resume", outcome = tracing::field::Empty, error.type = tracing::field::Empty, otel.status_code = tracing::field::Empty)
        }
        "relay.udp.validate" => {
            info_span!(parent: parent, "relay.udp.validate", outcome = tracing::field::Empty, error.type = tracing::field::Empty, otel.status_code = tracing::field::Empty)
        }
        _ => Span::none(),
    }
}

pub(super) fn mark_span(span: &Span, outcome: &'static str, error: Option<&'static str>) {
    span.record("outcome", outcome);
    if let Some(error) = error {
        span.record("error.type", error);
        span.record("otel.status_code", "ERROR");
    } else {
        span.record("otel.status_code", "OK");
    }
}

pub(super) const fn profile_name(profile: crate::domain::DataProfile) -> &'static str {
    match profile {
        crate::domain::DataProfile::Tcp => "tcp",
        crate::domain::DataProfile::Udp => "udp",
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use opentelemetry::metrics::MeterProvider as _;
    use opentelemetry_sdk::metrics::SdkMeterProvider;
    use tracing::{Id, Subscriber, span::Attributes};
    use tracing_subscriber::{Layer, layer::Context, prelude::*, registry::LookupSpan};

    use super::*;

    #[test]
    fn establishment_milestones_are_children_of_one_root() {
        let spans = CapturedSpans::default();
        let subscriber = tracing_subscriber::registry().with(spans.clone());
        let provider = SdkMeterProvider::builder().build();
        let metrics = Arc::new(RelayMetrics::new(&provider.meter("establishment-test")));

        tracing::subscriber::with_default(subscriber, || {
            let mut attempt = EstablishmentAttempt::start(PeerId::new(7), metrics);
            for milestone in [
                Milestone::Bootstrap,
                Milestone::JoinBegin,
                Milestone::JoinProof,
            ] {
                let span = attempt.milestone_span(milestone);
                mark_span(&span, "accepted", None);
            }
            attempt.set_route("join", "tcp");
            attempt.finish("accepted", None);
        });

        let spans = spans.0.lock().unwrap();
        assert_eq!(spans[0], ("relay.session.establish".to_owned(), None));
        assert_eq!(
            spans[1..],
            [
                (
                    "relay.bootstrap".to_owned(),
                    Some("relay.session.establish".to_owned())
                ),
                (
                    "relay.join.begin".to_owned(),
                    Some("relay.session.establish".to_owned())
                ),
                (
                    "relay.join.proof".to_owned(),
                    Some("relay.session.establish".to_owned())
                ),
            ]
        );
        assert!(spans.iter().all(|(name, _)| !matches!(
            name.as_str(),
            "relay.control" | "relay.data.dispatch" | "relay.probe.dispatch"
        )));
    }

    type CapturedSpan = (String, Option<String>);

    #[derive(Clone, Default)]
    struct CapturedSpans(Arc<Mutex<Vec<CapturedSpan>>>);

    impl<S> Layer<S> for CapturedSpans
    where
        S: Subscriber + for<'lookup> LookupSpan<'lookup>,
    {
        fn on_new_span(&self, attributes: &Attributes<'_>, id: &Id, context: Context<'_, S>) {
            let parent = attributes
                .parent()
                .and_then(|parent| context.span(parent))
                .or_else(|| context.span(id).and_then(|span| span.parent()))
                .map(|span| span.metadata().name().to_owned());
            self.0
                .lock()
                .unwrap()
                .push((attributes.metadata().name().to_owned(), parent));
        }
    }
}
