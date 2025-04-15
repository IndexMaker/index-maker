pub enum ServerEvent {
    NewIndexOrder,
    NewQuoteRequest,
    CancelIndexOrder,
    CancelQuoteRequest,
    AccountToCustody,
    CustodyToAccount,
}

pub trait Server {
    /// provide methods for sending FIX responses
    fn respond_with(&mut self, message: ());
}

#[cfg(test)]
pub mod test_util {

    use std::sync::Arc;

    use crate::core::functional::MultiObserver;

    use super::{Server, ServerEvent};

    pub struct MockServer {
        pub observers: MultiObserver<Arc<ServerEvent>>,
    }

    impl MockServer {
        pub fn new() -> Self {
            Self {
                observers: MultiObserver::new(),
            }
        }

        /// Receive FIX messages from clients
        pub fn start_server() {
            todo!()
        }

        /// Notify about FIX messages
        pub fn notify_fix_message(&self, _fix_message: ()) {
            // for each FIX message type there will be Server Event
            self.observers.publish_many(&Arc::new(
                ServerEvent::NewIndexOrder, // ...more event types
            ));
        }
    }

    impl Server for MockServer {
        /// provide methods for sending FIX responses
        fn respond_with(&mut self, _message: ()) {}
    }
}
