use chrono::{DateTime, Utc};

use crate::core::bits::{Address, Amount, ClientOrderId, PaymentId, Side, Symbol};

pub enum ServerEvent {
    NewIndexOrder {
        address: Address,
        client_order_id: ClientOrderId,
        payment_id: PaymentId,
        symbol: Symbol,
        side: Side,
        price: Amount,
        price_threshold: Amount,
        quantity: Amount,
        timestamp: DateTime<Utc>,
    },
    CancelIndexOrder {
        address: Address,
        client_order_id: ClientOrderId,
        payment_id: PaymentId,
        symbol: Symbol,
        quantity: Amount,
        timestamp: DateTime<Utc>,
    },
    NewQuoteRequest {
        address: Address,
        client_order_id: ClientOrderId,
        symbol: Symbol,
        side: Side,
        price: Amount,
        price_threshold: Amount,
        quantity: Amount,
        timestamp: DateTime<Utc>,
    },
    CancelQuoteRequest {
        address: Address,
        client_order_id: ClientOrderId,
        symbol: Symbol,
    },
    AccountToCustody,
    CustodyToAccount,
}

pub trait Server: Send + Sync {
    /// provide methods for sending FIX responses
    fn respond_with(&mut self, message: ());
}

#[cfg(test)]
pub mod test_util {

    use std::sync::Arc;

    use crate::core::functional::{IntoObservableMany, MultiObserver};

    use super::{Server, ServerEvent};

    pub struct MockServer {
        observers: MultiObserver<Arc<ServerEvent>>,
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
            //self.observers.publish_many(&Arc::new(
            //    ServerEvent::NewIndexOrder, // ...more event types
            //));
        }
    }

    impl Server for MockServer {
        /// provide methods for sending FIX responses
        fn respond_with(&mut self, _message: ()) {}
    }

    impl IntoObservableMany<Arc<ServerEvent>> for MockServer {
        fn get_multi_observer_mut(&mut self) -> &mut MultiObserver<Arc<ServerEvent>> {
            &mut self.observers
        }
    }
}
