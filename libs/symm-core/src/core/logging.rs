use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

pub fn log_init(filter: String) {
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| filter.into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();
}

/// Default initialize tracing log.
///
/// Must also import [`index_maker::core::logging::log_init`] function.
///
/// ## Using
/// ```
/// use index_maker::{core::logging::log_init, init_log}
///
/// init_log!();
/// tracing::info!("New order from: {}", "Bob");
/// tracing::debug!("Order details: {} @ {}", quantity, price);
/// tracing::warn!("Cannot find order: {}", order_id);
/// tracing::error!("Connection lost: {}", reason);
/// ```
///
/// # Configuring
/// Standard `RUST_LOG` environment variable can be used to configure, e.g.:
/// 
/// ```
/// export RUST_LOG="info"
/// ```
///
/// or more detailed:
/// ```
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
