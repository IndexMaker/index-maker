pub enum QuoteRequestEvent {
    NewQuoteRequest,
    CancelQuoteRequest,
}

/// manage index quotes, partly use solver to calcualte price
pub trait QuoteRequestManager {
    // provide method to respond to QR
    fn respond_quote(&mut self, quote: ());
}

#[cfg(test)]
pub mod test_util {
    use std::{collections::HashMap, sync::Arc};

    use parking_lot::RwLock;

    use crate::{
        core::{
            bits::BatchOrderId,
            functional::{IntoObservableSingle, PublishSingle, SingleObserver},
        },
        server::server::{Server, ServerEvent},
        solver::index_quote::IndexQuote,
    };

    use super::{QuoteRequestEvent, QuoteRequestManager};

    pub struct MockQuoteRequestManager {
        observer: SingleObserver<QuoteRequestEvent>,
        pub server: Arc<RwLock<dyn Server>>,
        pub quote_requests: HashMap<BatchOrderId, IndexQuote>,
    }
    impl MockQuoteRequestManager {
        pub fn new(server: Arc<RwLock<dyn Server>>) -> Self {
            Self {
                observer: SingleObserver::new(),
                server,
                quote_requests: HashMap::new(),
            }
        }

        fn notify_quote_request(&self, _quote_request: ()) {
            self.observer
                .publish_single(QuoteRequestEvent::NewQuoteRequest);
        }

        fn new_quote_request(&mut self, _quote_request: ()) {
            // 1. store QR
            //self.quote_requests.entry(key)
            // 2. notify about new QR
            self.notify_quote_request(());
        }

        /// receive QR
        pub fn handle_server_message(&mut self, notification: &ServerEvent) {
            match notification {
                ServerEvent::NewQuoteRequest {
                    chain_id: _,
                    address: _,
                    client_order_id: _,
                    symbol: _,
                    side: _,
                    collateral_amount: _,
                    timestamp: _,
                } => {
                    self.new_quote_request(());
                }
                ServerEvent::CancelQuoteRequest {
                    address: _,
                    client_order_id: _,
                    symbol: _,
                } => (),
                _ => (),
            }
        }
    }

    impl IntoObservableSingle<QuoteRequestEvent> for MockQuoteRequestManager {
        fn get_single_observer_mut(&mut self) -> &mut SingleObserver<QuoteRequestEvent> {
            &mut self.observer
        }
    }

    impl QuoteRequestManager for MockQuoteRequestManager {
        // provide method to respond to QR
        fn respond_quote(&mut self, _quote: ()) {
            todo!()
        }
    }
}
