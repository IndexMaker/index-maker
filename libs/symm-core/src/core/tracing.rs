use std::time::Duration;

use opentelemetry::trace::TracerProvider;
use opentelemetry::{global, KeyValue};
use opentelemetry_otlp::WithExportConfig;
use opentelemetry_sdk::trace::Sampler;
use opentelemetry_sdk::Resource;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, EnvFilter, Registry};

/// Function sets-up open-telemetry integrated with tracing
///
/// Provides:
/// 1. tracer for data recorded using spans
/// 2. logger for messages logged using warn!(), info!(), debug!(), ...
///
/// TODO: This is only example setup code. Need to evaluate.
///
pub fn setup_tracing() -> Result<(), Box<dyn std::error::Error>> {
    // --- OpenTelemetry Traces Setup (Existing) ---
    let otlp_trace_exporter = opentelemetry_otlp::SpanExporter::builder()
        .with_http()
        .with_endpoint("http://localhost:4318/v1/traces")
        .with_timeout(Duration::from_secs(3))
        .build()?;

    let tracer_provider = opentelemetry_sdk::trace::SdkTracerProvider::builder()
        .with_simple_exporter(otlp_trace_exporter)
        .with_sampler(Sampler::AlwaysOn)
        .with_resource(
            Resource::builder()
                .with_service_name("index-maker")
                .with_attribute(KeyValue::new("environment", "development"))
                .build(),
        )
        .build();

    let tracer = tracer_provider.tracer("index-maker");
    let telemetry_trace_layer = tracing_opentelemetry::layer().with_tracer(tracer);

    // --- OpenTelemetry Logs Setup (NEW) ---
    let otlp_log_exporter = opentelemetry_otlp::LogExporter::builder()
        .with_http()
        .with_endpoint("http://localhost:4318/v1/logs") // OTLP Logs endpoint (often different port or path)
        .with_timeout(Duration::from_secs(3))
        .build()?;

    let logger_provider = opentelemetry_sdk::logs::SdkLoggerProvider::builder()
        .with_simple_exporter(otlp_log_exporter)
        .with_resource(
            Resource::builder()
                .with_service_name("index-maker")
                .with_attribute(KeyValue::new("environment", "development"))
                .build(),
        )
        .build();

    // Bridge `tracing` events to OpenTelemetry Logs
    let telemetry_log_layer =
        opentelemetry_appender_tracing::layer::OpenTelemetryTracingBridge::new(&logger_provider);

    // --- Combine Layers into Subscriber ---
    let subscriber = Registry::default()
        .with(EnvFilter::from_default_env())
        .with(telemetry_trace_layer) // For spans -> traces
        .with(telemetry_log_layer) // For events -> logs (NEW)
        .with(tracing_subscriber::fmt::layer().compact()); // For console output

    // Set the subscriber as global default
    subscriber.init();

    // Set global OpenTelemetry providers (for direct OTel API usage)
    global::set_tracer_provider(tracer_provider);

    Ok(())
}
