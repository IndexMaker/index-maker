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
        if let Err(err) = self.inner.send_response(response) {
            tracing::warn!("Failed to respond with: {:?}", err);
        }
    }

    fn initialize_shutdown(&mut self) {
        tracing::info!("FIX Server shutdown initialized - closing server for new connections");
        self.inner.close_server();
    }
}
