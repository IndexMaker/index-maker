use std::time::Duration;

use opentelemetry::KeyValue;
use opentelemetry_otlp::WithExportConfig;
use opentelemetry_sdk::logs::{LoggerProviderBuilder, SdkLoggerProvider};
use opentelemetry_sdk::Resource;

use crate::tracing::defer_log::DeferLogProcessor;

pub fn create_otlp_log_layer() -> eyre::Result<SdkLoggerProvider> {
    // --- OpenTelemetry Logs Setup (NEW) ---
    let otlp_log_exporter = opentelemetry_otlp::LogExporter::builder()
        .with_http()
        .with_endpoint("http://localhost:4318/v1/logs") // OTLP Logs endpoint (often different port or path)
        .with_timeout(Duration::from_secs(3))
        .build()?;

    let mut log_processor = DeferLogProcessor::new();
    log_processor
        .start(otlp_log_exporter)
        .expect("Failed to start defer log processor");

    let logger_provider = LoggerProviderBuilder::default()
        .with_log_processor(log_processor)
        .with_resource(
            Resource::builder()
                .with_service_name("index-maker")
                .with_attribute(KeyValue::new("environment", "development"))
                .build(),
        )
        .build();

    Ok(logger_provider)
}
