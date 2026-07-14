use std::{env, io, sync::Arc, time::Duration};

use opentelemetry::{KeyValue, global, metrics::MeterProvider as _, trace::TracerProvider as _};
use opentelemetry_otlp::WithExportConfig as _;
use opentelemetry_sdk::{
    Resource,
    metrics::{Aggregation, Instrument, SdkMeterProvider, Stream},
    trace::{BatchConfigBuilder, BatchSpanProcessor, SdkTracerProvider},
};
use tracing::Subscriber;
use tracing_subscriber::{
    EnvFilter, Layer as _,
    fmt::format::{Format, Json, JsonFields, format},
    layer::SubscriberExt as _,
    registry::LookupSpan,
    util::SubscriberInitExt as _,
};

use crate::{config::TelemetryConfig, metrics::RelayMetrics};

const SERVICE_NAME: &str = "tractor-beam-relay";
const LOG_FORMAT_ENV: &str = "LOG_FORMAT";

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum LogFormat {
    #[default]
    Text,
    Json,
}

impl LogFormat {
    pub(crate) fn from_env() -> io::Result<Self> {
        match env::var(LOG_FORMAT_ENV) {
            Ok(value) => Self::parse(Some(&value)),
            Err(env::VarError::NotPresent) => Ok(Self::default()),
            Err(env::VarError::NotUnicode(_)) => Err(invalid_log_format("non-Unicode value")),
        }
    }

    fn parse(value: Option<&str>) -> io::Result<Self> {
        match value {
            None | Some("text") => Ok(Self::Text),
            Some("json") => Ok(Self::Json),
            Some(value) => Err(invalid_log_format(value)),
        }
    }
}

pub(crate) struct RelayTelemetry {
    pub(crate) metrics: Arc<RelayMetrics>,
    tracer_provider: Option<SdkTracerProvider>,
    meter_provider: Option<SdkMeterProvider>,
}

impl RelayTelemetry {
    pub(crate) fn init(
        config: Option<&TelemetryConfig>,
        log_format: LogFormat,
    ) -> io::Result<Self> {
        let Some(config) = config else {
            init_subscriber(
                tracing_subscriber::registry().with(default_filter()),
                log_format,
            )?;
            let meter = global::meter(SERVICE_NAME);
            return Ok(Self {
                metrics: Arc::new(RelayMetrics::new(&meter)),
                tracer_provider: None,
                meter_provider: None,
            });
        };

        let resource = Resource::builder()
            .with_attributes([
                KeyValue::new("service.name", SERVICE_NAME),
                KeyValue::new("service.version", crate::build_info::version_label()),
                KeyValue::new("service.instance.id", config.service_instance_id.clone()),
            ])
            .build();
        let span_exporter = opentelemetry_otlp::SpanExporter::builder()
            .with_tonic()
            .with_endpoint(config.otlp_endpoint.clone())
            .build()
            .map_err(init_error)?;
        let metric_exporter = opentelemetry_otlp::MetricExporter::builder()
            .with_tonic()
            .with_endpoint(config.otlp_endpoint.clone())
            .build()
            .map_err(init_error)?;
        let span_processor = BatchSpanProcessor::builder(span_exporter)
            .with_batch_config(
                BatchConfigBuilder::default()
                    .with_max_queue_size(2_048)
                    .with_max_export_batch_size(512)
                    .with_scheduled_delay(Duration::from_secs(5))
                    .build(),
            )
            .build();
        let tracer_provider = SdkTracerProvider::builder()
            .with_resource(resource.clone())
            .with_span_processor(span_processor)
            .build();
        let meter_provider = SdkMeterProvider::builder()
            .with_resource(resource)
            .with_periodic_exporter(metric_exporter)
            .with_view(duration_histogram_view)
            .build();
        let tracer = tracer_provider.tracer(SERVICE_NAME);
        let subscriber = tracing_subscriber::registry().with(default_filter()).with(
            tracing_opentelemetry::layer()
                .with_tracer(tracer)
                .with_filter(tracing_subscriber::filter::filter_fn(|metadata| {
                    metadata.is_span()
                })),
        );
        init_subscriber(subscriber, log_format)?;
        let meter = meter_provider.meter(SERVICE_NAME);
        Ok(Self {
            metrics: Arc::new(RelayMetrics::new(&meter)),
            tracer_provider: Some(tracer_provider),
            meter_provider: Some(meter_provider),
        })
    }

    pub(crate) async fn shutdown(self) {
        let (finished_tx, finished_rx) = tokio::sync::oneshot::channel();
        std::thread::spawn(move || {
            if let Some(provider) = self.meter_provider {
                let _ = provider.shutdown();
            }
            if let Some(provider) = self.tracer_provider {
                let _ = provider.shutdown();
            }
            let _ = finished_tx.send(());
        });
        let _ = tokio::time::timeout(Duration::from_secs(2), finished_rx).await;
    }
}

fn default_filter() -> EnvFilter {
    EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"))
}

fn init_subscriber<S>(subscriber: S, log_format: LogFormat) -> io::Result<()>
where
    S: Subscriber + for<'span> LookupSpan<'span> + Send + Sync + 'static,
{
    match log_format {
        LogFormat::Text => subscriber
            .with(tracing_subscriber::fmt::layer())
            .try_init()
            .map_err(init_error),
        LogFormat::Json => subscriber
            .with(
                tracing_subscriber::fmt::layer()
                    .event_format(json_event_format())
                    .fmt_fields(JsonFields::new())
                    .with_ansi(false),
            )
            .try_init()
            .map_err(init_error),
    }
}

fn json_event_format() -> Format<Json> {
    format().json().flatten_event(true)
}

fn duration_histogram_view(instrument: &Instrument) -> Option<Stream> {
    if !matches!(
        instrument.name(),
        "tractor_beam.relay.control.operation.duration"
            | "tractor_beam.relay.session.establishment.duration"
            | "tractor_beam.relay.data.dispatch.duration"
    ) {
        return None;
    }
    Stream::builder()
        .with_aggregation(Aggregation::ExplicitBucketHistogram {
            boundaries: vec![
                0.000_25, 0.000_5, 0.001, 0.002_5, 0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0,
                2.5,
            ],
            record_min_max: true,
        })
        .build()
        .ok()
}

fn init_error(error: impl std::fmt::Display) -> io::Error {
    io::Error::other(format!("failed to initialize telemetry: {error}"))
}

fn invalid_log_format(value: &str) -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidInput,
        format!("invalid {LOG_FORMAT_ENV} value {value:?}; expected \"text\" or \"json\""),
    )
}

#[cfg(test)]
mod tests {
    use std::{
        io::Write,
        sync::{Arc, Mutex},
    };

    use serde_json::Value;
    use tracing_subscriber::{fmt::MakeWriter, prelude::*};

    use super::*;

    #[test]
    fn log_format_defaults_to_text() {
        assert_eq!(LogFormat::parse(None).unwrap(), LogFormat::Text);
    }

    #[test]
    fn log_format_accepts_supported_values() {
        assert_eq!(LogFormat::parse(Some("text")).unwrap(), LogFormat::Text);
        assert_eq!(LogFormat::parse(Some("json")).unwrap(), LogFormat::Json);
    }

    #[test]
    fn log_format_rejects_unknown_values() {
        let error = LogFormat::parse(Some("pretty")).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
        assert!(error.to_string().contains("expected \"text\" or \"json\""));
    }

    #[test]
    fn json_format_flattens_structured_relay_fields_without_ansi() {
        let writer = CapturedWriter::default();
        let subscriber = tracing_subscriber::registry().with(
            tracing_subscriber::fmt::layer()
                .event_format(json_event_format())
                .fmt_fields(JsonFields::new())
                .with_ansi(false)
                .with_writer(writer.clone()),
        );

        tracing::subscriber::with_default(subscriber, || {
            let span = tracing::info_span!("relay.session.establish", session.operation = "join");
            let _entered = span.enter();
            tracing::info!(
                rooms = 0_u64,
                peers = 0_u64,
                missing_target = 2_u64,
                "relay stats"
            );
        });

        let output = writer.output();
        assert!(!output.contains('\u{1b}'));
        let event: Value = serde_json::from_str(output.trim()).unwrap();
        assert_eq!(event["level"], "INFO");
        assert_eq!(event["target"], module_path!());
        assert_eq!(event["message"], "relay stats");
        assert_eq!(event["rooms"], 0);
        assert_eq!(event["peers"], 0);
        assert_eq!(event["missing_target"], 2);
        assert!(event["timestamp"].is_string());
        assert_eq!(event["span"]["name"], "relay.session.establish");
    }

    #[derive(Clone, Default)]
    struct CapturedWriter(Arc<Mutex<Vec<u8>>>);

    impl CapturedWriter {
        fn output(&self) -> String {
            String::from_utf8(self.0.lock().unwrap().clone()).unwrap()
        }
    }

    impl Write for CapturedWriter {
        fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buffer);
            Ok(buffer.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    impl<'writer> MakeWriter<'writer> for CapturedWriter {
        type Writer = Self;

        fn make_writer(&'writer self) -> Self::Writer {
            self.clone()
        }
    }
}
