use std::sync::Arc;

use super::config::ConfigBuildError;
use derive_builder::Builder;
use parking_lot::RwLock;

use symm_core::core::functional::{
    IntoObservableManyVTable,
    MultiObserver, NotificationHandler, NotificationHandlerOnce, PublishMany,
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

    pub fn publish_event(&self, event: &Arc<ServerEvent>) {
        self.observer.publish_many(event);
    }
}

impl Server for SimpleServer {
    fn respond_with(&mut self, response: ServerResponse) {
        tracing::info!("Received response: {:?}", response);
    }
}

impl IntoObservableManyVTable<Arc<ServerEvent>> for SimpleServer {
    fn add_observer(&mut self, observer: Box<dyn NotificationHandler<Arc<ServerEvent>>>) {
        self.observer.add_observer(observer);
    }
}

#[derive(Debug, Clone)]
pub enum ServerKind {
    Simple,
}

pub enum ServerHandoffEvent {
    Simple(Arc<RwLock<SimpleServer>>),
}

#[derive(Clone, Builder)]
#[builder(
    pattern = "owned",
    build_fn(name = "try_build", error = "ConfigBuildError")
)]
pub struct ServerConfig {
    #[builder(setter(into, strip_option), default)]
    pub server_kind: Option<ServerKind>,

    #[builder(setter(skip))]
    pub(crate) server: Option<Arc<RwLock<dyn Server + Send + Sync>>>,
}

impl ServerConfig {
    #[must_use]
    pub fn builder() -> ServerConfigBuilder {
        ServerConfigBuilder::default()
    }

    pub fn expect_server_cloned(&self) -> Arc<RwLock<dyn Server + Send + Sync>> {
        self.server.clone().ok_or(()).expect("Failed to get server")
    }
}

impl ServerConfigBuilder {
    pub fn build(
        self,
        handoff: impl NotificationHandlerOnce<ServerHandoffEvent>,
    ) -> Result<ServerConfig, ConfigBuildError> {
        let mut config = self.try_build()?;

        config.server.replace(match config.server_kind.take() {
            Some(ServerKind::Simple) => {
                let server = Arc::new(RwLock::new(SimpleServer::new()));
                handoff.handle_notification(ServerHandoffEvent::Simple(server.clone()));
                server
            }
            None => Err(ConfigBuildError::UninitializedField("server_kind"))?,
        });

        Ok(config)
    }
}
