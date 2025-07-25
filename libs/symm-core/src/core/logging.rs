use std::{
    sync::{Once, OnceLock},
    time::{SystemTime, UNIX_EPOCH},
};
use tracing_appender::rolling::{self, Builder};
use tracing_subscriber::{
    fmt::Layer, layer::SubscriberExt, util::SubscriberInitExt
};

use crate::tracing::{otlp_log::create_otlp_log_layer, otlp_tracing::create_otlp_trace_layer};

static INIT_LOG: Once = Once::new();
static LOG_GUARD: OnceLock<tracing_appender::non_blocking::WorkerGuard> = OnceLock::new();

pub fn log_init(
    filter: String,
    log_path: Option<String>,
    disable_terminal_log: bool,
    otlp_trace_url: Option<Option<String>>,
    otlp_log_url: Option<Option<String>>,
    batch_size: Option<usize>,
) {
    INIT_LOG.call_once(|| {
        // Set up the terminal output layer
        let terminal_layer = if !disable_terminal_log {
            // Enable ANSI colors for terminal output
            let terminal_layer = Layer::new().with_writer(std::io::stdout).with_ansi(true);
            Some(terminal_layer)
        } else {
            None
        };

        // Set up the global filter from RUST_LOG or fallback to the provided filter
        let env_filter =
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| filter.into());

        let file_layer = if let Some(log_directory) = log_path {
            // Generate a unique ID for the log filename
            let unique_id = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("Time went backwards")
                .as_secs();

            // Set file rotation in tracing_appender
            let filename_suffix = format!("{}.log", unique_id);
            let file_appender = Builder::new()
                .rotation(rolling::Rotation::DAILY)
                .filename_prefix("index-maker")
                .filename_suffix(&filename_suffix)
                .build(log_directory)
                .expect("Failed to build rolling file appender");

            let (non_blocking_file_writer, guard) = tracing_appender::non_blocking(file_appender);

            // Store the guard on a static variable, to persists for the program lifetime
            LOG_GUARD.set(guard).expect("Failed to set log guard");

            // Set up the file output layer
            let file_layer = Layer::new()
                .with_writer(non_blocking_file_writer)
                .with_ansi(false); // Disable ANSI colors for file output
            Some(file_layer)
        } else {
            None
        };

        let otlp_trace_layer = if let Some(url) = otlp_trace_url {
            let tracer =
                create_otlp_trace_layer(url, batch_size).expect("Failed to create Open-Telemetry trace layer");
            let telemetry_trace_layer = tracing_opentelemetry::layer().with_tracer(tracer);
            Some(telemetry_trace_layer)
        } else {
            None
        };

        let otlp_log_layer = if let Some(url) = otlp_log_url {
            let logger_provider =
                create_otlp_log_layer(url, batch_size).expect("Failed to create Open-Telemetry log layer");
            let telemetry_log_layer =
                opentelemetry_appender_tracing::layer::OpenTelemetryTracingBridge::new(
                    &logger_provider,
                );
            Some(telemetry_log_layer)
        } else {
            None
        };

        // Combine the layers into a subscriber
        let registry = tracing_subscriber::registry()
            .with(env_filter) // Apply the filter globally to both layers
            .with(terminal_layer)
            .with(file_layer)
            .with(otlp_trace_layer)
            .with(otlp_log_layer);

        registry.init();
    });
}

/// Default initialize tracing log.
///
/// Must also import [`symm_core::core::logging::log_init`] function.
///
/// ## Using
/// ```rust
/// use symm_core::{core::logging::log_init, init_log};
///
/// init_log!();
/// tracing::info!("New order from: {}", "Bob");
/// tracing::debug!("Order details: {} @ {}", 100.0, 200.0);
/// tracing::warn!("Cannot find order: {}", "O-123456");
/// tracing::error!("Connection lost: {}", "Failed to connect");
/// ```
///
/// # Configuring
/// Standard `RUST_LOG` environment variable can be used to configure, e.g.:
///
/// ```bash
/// export RUST_LOG="info"
/// ```
///
/// or more detailed:
/// ```bash
/// export RUST_LOG="my_module_name=debug"
/// ```
///
/// For more details check [Logging Directives Documentation](https://docs.rs/tracing-subscriber/latest/tracing_subscriber/filter/struct.EnvFilter.html#directives)
///
#[macro_export]
macro_rules! init_log {
    () => {
        log_init(
            format!("{}=info", env!("CARGO_CRATE_NAME")),
            None,
            false,
            None,
            None,
            None,
        );
    };
}
