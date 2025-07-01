use symm_core::core::functional::{
    IntoObservableManyVTable, MultiObserver, NotificationHandler, PublishMany,
};

pub struct ObserverPlugin<R> {
    observer: MultiObserver<R>,
}

impl<R> ObserverPlugin<R> {
    pub fn new() -> Self {
        Self {
            observer: MultiObserver::new(),
        }
    }
    
    /// handle_server_message
    ///
    /// Processes an incoming server request by publishing it to all registered observers for processing.
    pub fn publish_request(&self, request: &R) {
        self.observer.publish_many(request);
    }
}

impl<R> IntoObservableManyVTable<R> for ObserverPlugin<R> {
    fn add_observer(&mut self, observer: Box<dyn NotificationHandler<R>>) {
        self.observer.add_observer(observer);
    }
}
