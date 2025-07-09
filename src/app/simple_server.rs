use std::sync::Arc;

use super::config::ConfigBuildError;
use derive_builder::Builder;
use eyre::{OptionExt, Result};
use parking_lot::RwLock;
use crate::app::solver::ServerConfig;

use symm_core::core::functional::{
    IntoObservableManyVTable, MultiObserver, NotificationHandler, PublishMany,
};

use crate::server::server::{Server, ServerEvent, ServerResponse};

pub struct SimpleServer {
    observer: MultiObserver<Arc<ServerEvent>>,
}

impl SimpleServer {
    pub fn new() -> Self {
        Self {
            observer: MultiObserver::new(),
        }
    }
}

impl Server for SimpleServer {
    fn respond_with(&mut self, response: ServerResponse) {
        tracing::info!("Received response: {:?}", response);
    }

    fn publish_event(&mut self, event: &Arc<ServerEvent>) {
        self.observer.publish_many(event);
    }
}

impl IntoObservableManyVTable<Arc<ServerEvent>> for SimpleServer {
    fn add_observer(&mut self, observer: Box<dyn NotificationHandler<Arc<ServerEvent>>>) {
        self.observer.add_observer(observer);
    }
}

#[derive(Clone, Builder)]
#[builder(
    pattern = "owned",
    build_fn(name = "try_build", error = "ConfigBuildError")
)]
pub struct SimpleServerConfig {
    #[builder(setter(skip))]
    simple_server: Option<Arc<RwLock<SimpleServer>>>,
}

impl SimpleServerConfig {
    #[must_use]
    pub fn builder() -> SimpleServerConfigBuilder {
        SimpleServerConfigBuilder::default()
    }

    pub fn expect_simple_server_cloned(&self) -> Arc<RwLock<SimpleServer>> {
        self.simple_server
            .clone()
            .ok_or(())
            .expect("Failed to get simple server")
    }

    pub fn try_get_simple_server_cloned(&self) -> Result<Arc<RwLock<SimpleServer>>> {
        self.simple_server
            .clone()
            .ok_or_eyre("Failed to get simple server")
    }
}

impl ServerConfig for SimpleServerConfig {
    fn expect_server_cloned(&self) -> Arc<RwLock<dyn Server + Send + Sync>> {
        self.expect_simple_server_cloned()
    }

    fn try_get_server_cloned(&self) -> Result<Arc<RwLock<dyn Server + Send + Sync>>> {
        self.try_get_simple_server_cloned()
            .map(|x| x as Arc<RwLock<dyn Server + Send + Sync>>)
    }
}

impl SimpleServerConfigBuilder {
    pub fn build_arc(self) -> Result<Arc<SimpleServerConfig>, ConfigBuildError> {
        let mut config = self.try_build()?;

        config
            .simple_server
            .replace(Arc::new(RwLock::new(SimpleServer::new())));

        Ok(Arc::new(config))
    }
}
