use std::sync::Arc;

use super::config::ConfigBuildError;
use derive_builder::Builder;
use eyre::{OptionExt, Result};
use parking_lot::RwLock;

use crate::app::solver::ServerConfig;
use crate::server::fix::server::Server as FixServer;
use crate::server::server::Server;

#[derive(Clone, Builder)]
#[builder(
    pattern = "owned",
    build_fn(name = "try_build", error = "ConfigBuildError")
)]

pub struct AxumServerConfig {
    #[builder(setter(into, strip_option), default)]
    pub address: Option<String>,

    #[builder(setter(skip))]
    pub fix_server: Option<Arc<RwLock<FixServer>>>,
}

impl AxumServerConfig {
    #[must_use]
    pub fn builder() -> AxumServerConfigBuilder {
        AxumServerConfigBuilder::default()
    }

    pub fn expect_server_cloned(&self) -> Arc<RwLock<FixServer>> {
        self.fix_server
            .clone()
            .ok_or(())
            .expect("Failed to get axum server")
    }

    pub fn try_get_server_cloned(&self) -> Result<Arc<RwLock<FixServer>>> {
        self.fix_server
            .clone()
            .ok_or_eyre("Failed to get axum server")
    }

    pub async fn start(&self) -> Result<()> {
        let address = self
            .address
            .clone()
            .unwrap_or(String::from("127.0.0.1:3000"));

        self.fix_server
            .as_ref()
            .ok_or_eyre("Server not configured")?
            .read()
            .start(address)
    }

    pub async fn stop(&self) -> Result<()> {
        self.fix_server
            .as_ref()
            .ok_or_eyre("Server not configured")?
            .read()
            .stop()
            .await
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
            .fix_server
            .replace(Arc::new(RwLock::new(FixServer::new())));

        Ok(Arc::new(config))
    }
}
