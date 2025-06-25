use std::sync::Arc;

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
