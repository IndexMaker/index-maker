use std::time::Duration;

use opentelemetry::trace::TracerProvider;
use opentelemetry::{global, KeyValue};
use opentelemetry_otlp::WithExportConfig;
use opentelemetry_sdk::trace::{Sampler, SdkTracer};
use opentelemetry_sdk::Resource;

use crate::tracing::defer_span::DeferSpanProcessor;

pub fn create_otlp_trace_layer(url: String) -> eyre::Result<SdkTracer> {
    // --- OpenTelemetry Traces Setup (Existing) ---
    let otlp_trace_exporter = opentelemetry_otlp::SpanExporter::builder()
        .with_http()
        .with_endpoint(if url == "default" {
            "http://localhost:4318/v1/traces"
        } else {
            &url
        })
        .with_timeout(Duration::from_secs(3))
        .build()?;

    let mut span_processor = DeferSpanProcessor::new(512);
    span_processor
        .start(otlp_trace_exporter)
        .expect("Failed to start defer span processor");

    let tracer_provider = opentelemetry_sdk::trace::SdkTracerProvider::builder()
        .with_span_processor(span_processor)
        .with_sampler(Sampler::AlwaysOn)
        .with_resource(
            Resource::builder()
                .with_service_name("index-maker")
                .with_attribute(KeyValue::new("service.version", "1.0.0"))
                .with_attribute(KeyValue::new("deployment.environment", "production"))
                .build(),
        )
        .build();

    let tracer = tracer_provider.tracer("index-maker");

    global::set_tracer_provider(tracer_provider);

    Ok(tracer)
}
