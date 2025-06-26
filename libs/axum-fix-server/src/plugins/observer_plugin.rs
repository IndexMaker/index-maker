use symm_core::core::functional::{NotificationHandlerOnce, PublishSingle, SingleObserver};

pub struct ObserverPlugin<R> {
    observer: SingleObserver<R>,
}

impl<R> ObserverPlugin<R> {
    pub fn new() -> Self {
        Self {
            observer: SingleObserver::new(),
        }
    }

    pub fn set_observer_closure(&mut self, closure: impl NotificationHandlerOnce<R> + 'static) {
        self.observer.set_observer_fn(closure);
    }

    // pub fn get_multi_observer_mut(&mut self) -> &mut SingleObserver<R> {
    //     &mut self.observer
    // }

    /// handle_server_message
    ///
    /// Processes an incoming server request by publishing it to all registered observers for processing.
    pub fn publish_request(&self, request: R) {
        self.observer.publish_single(request);
    }


}