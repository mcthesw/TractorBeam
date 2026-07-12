use std::{io, sync::Arc, time::Duration};

use opentelemetry::{KeyValue, global, metrics::MeterProvider as _, trace::TracerProvider as _};
use opentelemetry_otlp::WithExportConfig as _;
use opentelemetry_sdk::{
    Resource,
    metrics::{Aggregation, Instrument, SdkMeterProvider, Stream},
    trace::{BatchConfigBuilder, BatchSpanProcessor, SdkTracerProvider},
};
use tracing_subscriber::{Layer as _, layer::SubscriberExt as _, util::SubscriberInitExt as _};

use crate::{config::TelemetryConfig, metrics_v2::RelayMetricsV2};

const SERVICE_NAME: &str = "tractor-beam-relay";

pub(crate) struct RelayTelemetry {
    pub(crate) metrics: Arc<RelayMetricsV2>,
    tracer_provider: Option<SdkTracerProvider>,
    meter_provider: Option<SdkMeterProvider>,
}

impl RelayTelemetry {
    pub(crate) fn init(config: Option<&TelemetryConfig>) -> io::Result<Self> {
        let Some(config) = config else {
            tracing_subscriber::fmt().try_init().map_err(init_error)?;
            let meter = global::meter(SERVICE_NAME);
            return Ok(Self {
                metrics: Arc::new(RelayMetricsV2::new(&meter, "", 0.0)),
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
        tracing_subscriber::registry()
            .with(tracing_subscriber::fmt::layer())
            .with(
                tracing_opentelemetry::layer()
                    .with_tracer(tracer)
                    .with_filter(tracing_subscriber::filter::filter_fn(|metadata| {
                        metadata.is_span()
                    })),
            )
            .try_init()
            .map_err(init_error)?;
        let meter = meter_provider.meter(SERVICE_NAME);
        Ok(Self {
            metrics: Arc::new(RelayMetricsV2::new(
                &meter,
                &config.service_instance_id,
                config.data_trace_sample_ratio,
            )),
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

fn duration_histogram_view(instrument: &Instrument) -> Option<Stream> {
    if !matches!(
        instrument.name(),
        "tractor_beam.relay.control.operation.duration"
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
