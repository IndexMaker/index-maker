use std::sync::Arc;

use super::config::ConfigBuildError;
use derive_builder::Builder;
use eyre::{OptionExt, Result};

use axum_fix_server::server::Server as AxumFixServer;
use parking_lot::RwLock;
use symm_core::core::functional::{IntoObservableManyVTable, NotificationHandler};
use tokio::sync::RwLock as TokioLock;

use crate::app::solver::ServerConfig;
use crate::server::server::{Server, ServerEvent, ServerResponse};
use crate::server::server_plugin::ServerPlugin;

pub struct ServerImpl(Arc<TokioLock<AxumFixServer<ServerResponse, ServerPlugin>>>);

#[derive(Clone, Builder)]
#[builder(
    pattern = "owned",
    build_fn(name = "try_build", error = "ConfigBuildError")
)]

pub struct AxumServerConfig {
    #[builder(setter(into, strip_option), default)]
    pub address: Option<String>,

    #[builder(setter(skip))]
    pub axum_server: Option<Arc<RwLock<ServerImpl>>>,
}

impl AxumServerConfig {
    #[must_use]
    pub fn builder() -> AxumServerConfigBuilder {
        AxumServerConfigBuilder::default()
    }

    pub fn expect_server_cloned(&self) -> Arc<RwLock<ServerImpl>> {
        self.axum_server
            .clone()
            .ok_or(())
            .expect("Failed to get axum server")
    }

    pub fn try_get_server_cloned(&self) -> Result<Arc<RwLock<ServerImpl>>> {
        self.axum_server
            .clone()
            .ok_or_eyre("Failed to get axum server")
    }

    pub async fn start(&self) -> Result<()> {
        if let Some(server) = &self.axum_server {
            let address = self
                .address
                .clone()
                .unwrap_or(String::from("127.0.0.1:3000"));
            server.read().0.read().await.start_server(address);
        }
        Ok(())
    }

    pub async fn stop(&self) -> Result<()> {
        if let Some(server) = &self.axum_server {
            server.read().0.write().await.stop_server();
        }
        Ok(())
    }
}

impl IntoObservableManyVTable<Arc<ServerEvent>> for ServerImpl {
    fn add_observer(&mut self, observer: Box<dyn NotificationHandler<Arc<ServerEvent>>>) {
        let server = self.0.clone();
        tokio::spawn(async move {
            server.write().await.plugin.add_observer(observer);
        });
    }
}

impl Server for ServerImpl {
    fn respond_with(&mut self, response: ServerResponse) {
        let server = self.0.clone();
        tokio::spawn(async move {
            if let Err(err) = server.write().await.send_response(response){
                tracing::warn!("Failed to respond with: {:?}", err);
            }
        });
    }
}

impl ServerConfig for AxumServerConfig {
    fn expect_server_cloned(&self) -> Arc<RwLock<dyn Server + Send + Sync>> {
        self.expect_server_cloned()
    }

    fn try_get_server_cloned(&self) -> Result<Arc<RwLock<dyn Server + Send + Sync>>> {
        self.try_get_server_cloned()
            .map(|x| x as Arc<RwLock<dyn Server + Send + Sync>>)
    }
}

impl AxumServerConfigBuilder {
    pub fn build_arc(self) -> Result<Arc<AxumServerConfig>, ConfigBuildError> {
        let mut config = self.try_build()?;

        config
            .axum_server
            .replace(Arc::new(RwLock::new(ServerImpl(AxumFixServer::new_arc(
                ServerPlugin::new(),
            )))));

        Ok(Arc::new(config))
    }
}
