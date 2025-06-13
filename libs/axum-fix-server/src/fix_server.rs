use std::sync::Arc;

use index_maker::{
    core::functional::{IntoObservableMany, MultiObserver, PublishMany},
    server::server::{Server, ServerEvent, ServerResponse},
};
use tokio::spawn;

/// fix server over web socket, should receive fix messages, emit events into
/// rest of the IndexMaker, and take responses from the IndexMaker

pub struct FixServer {
    observer: MultiObserver<Arc<ServerEvent>>,
    // add more data
}

impl FixServer {
    pub fn new() -> Self {
        Self {
            observer: MultiObserver::new(),
        }
    }

    pub fn start_server(&mut self) {
        spawn(async {
            // some work
        });
        todo!("Start FIX server")
    }

    /// call this method interanlly to publish ServerEvent received from FIX client
    fn notify_server_event(&self, server_event: Arc<ServerEvent>) {
        self.observer.publish_many(&server_event);
    }
}

impl Server for FixServer {
    fn respond_with(&mut self, response: ServerResponse) {
        todo!("Send response back to the user");
    }
}

impl IntoObservableMany<Arc<ServerEvent>> for FixServer {
    fn get_multi_observer_mut(&mut self) -> &mut MultiObserver<Arc<ServerEvent>> {
        &mut self.observer
    }
}
