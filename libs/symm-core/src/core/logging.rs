use std::sync::Once;

use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

static INIT_LOG: Once = Once::new();

pub fn log_init(filter: String) {
    INIT_LOG.call_once(|| {
        tracing_subscriber::registry()
            .with(
                tracing_subscriber::EnvFilter::try_from_default_env()
                    .unwrap_or_else(|_| filter.into()),
            )
            .with(tracing_subscriber::fmt::layer())
            .init();
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
        log_init(format!("{}=info", env!("CARGO_CRATE_NAME")));
    };
}
