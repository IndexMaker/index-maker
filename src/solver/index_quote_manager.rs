pub enum QuoteRequestEvent {
    NewQuoteRequest,
    CancelQuoteRequest,
}
pub trait QuoteRequestObserver {
    fn handle_quote_request(&self, event: QuoteRequestEvent);
}

/// manage index quotes, partly use solver to calcualte price
pub trait QuoteRequestManager {}

#[cfg(test)]
pub mod test_util {
    use std::sync::Arc;

    use parking_lot::RwLock;

    use crate::{
        core::functional::SingleObserver,
        server::server::{Server, ServerEvent},
    };

    use super::{QuoteRequestEvent, QuoteRequestManager};

    pub struct MockQuoteRequestManager {
        pub observer: SingleObserver<QuoteRequestEvent>,
        pub server: Arc<RwLock<dyn Server>>,
    }
    impl MockQuoteRequestManager {
        pub fn new(server: Arc<RwLock<dyn Server>>) -> Self {
            Self {
                observer: SingleObserver::new(),
                server,
            }
        }

        pub fn handle_server_message(&self, _notification: &ServerEvent) {
            todo!()
        }
    }

    impl QuoteRequestManager for MockQuoteRequestManager {}
}
