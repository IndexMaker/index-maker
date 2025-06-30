use std::sync::{Arc, Weak};

use super::config::ConfigBuildError;
use derive_builder::Builder;
use eyre::{OptionExt, Result};


use axum_fix_server::server::Server as AxumFixServer;
use tokio::sync::RwLock;

use crate::server::requests::FixRequest;
use crate::server::responses::FixResponse;
use crate::server::example_plugin::ExamplePlugin;
use crossbeam::channel::{unbounded, Receiver};

pub struct AxumServer {
    receiver: Receiver<FixRequest>,
    inner: Arc<RwLock<AxumFixServer<FixResponse, ExamplePlugin<FixRequest, FixResponse>>>>,
    weak: Weak<RwLock<AxumFixServer<FixResponse, ExamplePlugin<FixRequest, FixResponse>>>>,
}

impl AxumServer {
    // pub fn new() -> Self {
    //     let mut plugin = ExamplePlugin::<FixRequest, FixResponse>::new();
    //     // plugin.set_observer_plugin_callback(move |e: Request| {
    //     //     //handle_server_event(&e);
    //     //     //event_tx.send(SolverEvents::Message(e)).unwrap();
    //     // });

    //     let fix_server = AxumFixServer::new_arc(plugin);
    //     Self {
    //         observer: MultiObserver::new(),
    //         inner: fix_server,
    //     }
    // }

    pub async fn start_server(&self, address: &'static str) {
        // Starting the server and wainting connections
        self.inner.read().await.start_server(address);
    }

    pub async fn close_server(&self) {
        // Stops theserver from accepting new connections
        self.inner.write().await.close_server();
    }

    pub async fn stop_server(&self) {
        // Closes all server sessions
        self.inner.write().await.stop_server();
    }

}

// impl Default for AxumServer {
//     fn default() -> Self {
//         Self::new()
//     }
// }

// impl Server for AxumServer {
//     fn respond_with(&mut self, response: ServerResponse) {
//         tracing::info!("Received response: {:?}", response);
//         // Forward response to the inner AxumFixServer if needed
//         // self.inner.respond_with(response);
//     }
// }

#[derive(Clone, Builder)]
#[builder(
    pattern = "owned",
    build_fn(name = "try_build", error = "ConfigBuildError")
)]
pub struct AxumServerConfig {
    #[builder(setter(skip))]
    pub axum_server: Option<Arc<RwLock<AxumServer>>>,
    #[builder(setter(skip))]
    pub address: String,
}

pub trait ServerConfig {
    fn expect_server_cloned(&self) -> Arc<RwLock<AxumServer>>;
    fn try_get_server_cloned(&self) -> Result<Arc<RwLock<AxumServer>>>;
}

impl AxumServerConfig {
    #[must_use]
    pub fn builder() -> AxumServerConfigBuilder {
        AxumServerConfigBuilder::default()
    }

    pub fn expect_server_cloned(&self) -> Arc<RwLock<AxumServer>> {
        self.axum_server
            .clone()
            .ok_or(())
            .expect("Failed to get axum server")
    }

    pub fn try_get_server_cloned(&self) -> Result<Arc<RwLock<AxumServer>>> {
        self.axum_server
            .clone()
            .ok_or_eyre("Failed to get axum server")
    }
}

impl ServerConfig for AxumServerConfig {
    fn expect_server_cloned(&self) -> Arc<RwLock<AxumServer>> {
        self.expect_server_cloned()
    }

    fn try_get_server_cloned(&self) -> Result<Arc<RwLock<AxumServer>>> {
        self.try_get_server_cloned()
            .map(|x| x as Arc<RwLock<AxumServer>>)
    }
}

impl AxumServerConfigBuilder {
    pub fn build_arc(self) -> Result<Arc<AxumServerConfig>, ConfigBuildError> {
        let mut config = self.try_build()?;

        // Set up the plugin as in the example
        let (event_tx, event_rx) = unbounded::<FixRequest>(); // Assuming SolverEvents is defined
        let mut plugin = ExamplePlugin::<FixRequest, FixResponse>::new(); // Assuming Request and Response are defined
        plugin.set_observer_plugin_callback(move |e: FixRequest| {
            event_tx.send(e);
        });

        // Create the AxumFixServer with the plugin
        let fix_server = AxumFixServer::new_arc(plugin);
        let fix_server_weak = Arc::downgrade(&fix_server);

        // Wrap the server in AxumServer
        let axum_server = AxumServer {
            receiver: event_rx,
            inner: fix_server,
            weak: fix_server_weak,
        };

        config.axum_server.replace(Arc::new(RwLock::new(axum_server)));

        Ok(Arc::new(config))
    }
}