use std::time::Duration;

use opentelemetry::KeyValue;
use opentelemetry_otlp::WithExportConfig;
use opentelemetry_sdk::logs::{LoggerProviderBuilder, SdkLoggerProvider};
use opentelemetry_sdk::Resource;

use crate::tracing::defer_log::DeferLogProcessor;

const DEFAULT_OTLP_LOGS_URL: &str = "http://localhost:4318/v1/logs";

pub fn create_otlp_log_layer(
    url: Option<String>,
    batch_size: Option<usize>,
) -> eyre::Result<SdkLoggerProvider> {
    let otlp_log_exporter = opentelemetry_otlp::LogExporter::builder()
        .with_http()
        .with_endpoint(url.unwrap_or(DEFAULT_OTLP_LOGS_URL.into()))
        .with_timeout(Duration::from_secs(3))
        .build()?;

    let mut log_processor = DeferLogProcessor::new(batch_size.unwrap_or(512));
    log_processor
        .start(otlp_log_exporter)
        .expect("Failed to start defer log processor");

    let logger_provider = LoggerProviderBuilder::default()
        .with_log_processor(log_processor)
        .with_resource(
            Resource::builder()
                .with_service_name("index-maker")
                .with_attribute(KeyValue::new("service.version", "1.0.0"))
                .with_attribute(KeyValue::new("deployment.environment", "production"))
                .build(),
        )
        .build();

    Ok(logger_provider)
}
