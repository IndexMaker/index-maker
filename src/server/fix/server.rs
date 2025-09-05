use std::sync::Arc;

use axum_fix_server::server::Server as AxumFixServer;
use eyre::Result;
use symm_core::core::functional::{IntoObservableManyVTable, NotificationHandler};

use crate::server::{
    fix::rate_limit_config::FixRateLimitConfig,
    fix::server_plugin::ServerPlugin,
    server::{Server as ServerInterface, ServerEvent, ServerResponse},
};

pub struct Server {
    inner: AxumFixServer<ServerResponse, ServerPlugin>,
}

impl Server {
    pub fn new() -> Self {
        Self::new_with_rate_limiting(FixRateLimitConfig::default())
    }

    pub fn new_with_rate_limiting(rate_limit_config: FixRateLimitConfig) -> Self {
        Self {
            inner: AxumFixServer::new(ServerPlugin::new(rate_limit_config)),
        }
    }

    pub fn start(&self, address: String) -> Result<()> {
        self.inner.start_server(address)
    }

    pub async fn stop(&self) -> Result<()> {
        self.inner.stop_server().await
    }
}

impl IntoObservableManyVTable<Arc<ServerEvent>> for Server {
    fn add_observer(&mut self, observer: Box<dyn NotificationHandler<Arc<ServerEvent>>>) {
        self.inner
            .with_plugin_mut(|plugin| plugin.add_observer(observer))
    }
}

impl ServerInterface for Server {
    fn respond_with(&mut self, response: ServerResponse) {
        // Pull identifiers in one place
        let (chain_id_opt, address_opt, client_order_id_opt, client_quote_id_opt) =
            response.telemetry_ids();

        // Materialize owned strings so &str borrows are stable during the event! macro
        let chain_attr: i64 = chain_id_opt.map(|v| v as i64).unwrap_or(-1);

        let address_str: String = match &address_opt {
            Some(a) => format!("{}", a),
            None => String::from("none"),
        };

        let client_order_id_str: String = match &client_order_id_opt {
            Some(coid) => String::from(coid.as_str()),
            None => String::from("none"),
        };

        let client_quote_id_str: String = match &client_quote_id_opt {
            Some(cqid) => String::from(cqid.as_str()),
            None => String::from("none"),
        };

        // Emit the OTLP-friendly event with stable &str borrows
        tracing::event!(
            tracing::Level::INFO,
            otlp_kind = "fix_response",
            chain_id = chain_attr,
            address = address_str.as_str(),
            client_order_id = client_order_id_str.as_str(),
            client_quote_id = client_quote_id_str.as_str()
        );

        // Send the FIX response; warn (don't panic) on failure
        if let Err(err) = self.inner.send_response(response) {
            tracing::warn!("Failed to respond with: {:?}", err);
        }
    }

    fn initialize_shutdown(&mut self) {
        tracing::info!("FIX Server shutdown initialized - closing server for new connections");
        self.inner.close_server();
    }
}
