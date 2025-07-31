use std::time::Duration;

use opentelemetry::trace::TracerProvider;
use opentelemetry::{global, KeyValue};
use opentelemetry_otlp::WithExportConfig;
use opentelemetry_sdk::trace::{Sampler, SdkTracer};
use opentelemetry_sdk::Resource;

use crate::tracing::defer_span::DeferSpanProcessor;

const DEFAULT_OTLP_TRACES_URL: &str = "http://localhost:4318/v1/traces";

pub fn create_otlp_trace_layer(
    url: Option<String>,
    batch_size: Option<usize>,
) -> eyre::Result<SdkTracer> {
    let otlp_trace_exporter = opentelemetry_otlp::SpanExporter::builder()
        .with_http()
        .with_endpoint(url.unwrap_or(DEFAULT_OTLP_TRACES_URL.into()))
        .with_timeout(Duration::from_secs(3))
        .build()?;

    let mut span_processor = DeferSpanProcessor::new(batch_size.unwrap_or(512));
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
