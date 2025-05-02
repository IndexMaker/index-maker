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

pub enum ServerResponse {
    NewIndexOrderAck {
        address: Address,
        client_order_id: ClientOrderId,
        timestamp: DateTime<Utc>,
    },
    IndexOrderFill {
        address: Address,
        client_order_id: ClientOrderId,
        filled_quantity: Amount,
        quantity_remaining: Amount,
        timestamp: DateTime<Utc>,
    },
}

pub trait Server: Send + Sync {
    /// provide methods for sending FIX responses
    fn respond_with(&mut self, response: ServerResponse);
}

#[cfg(test)]
pub mod test_util {

    use std::sync::Arc;

    use crate::core::functional::{
        IntoObservableMany, MultiObserver, PublishMany, PublishSingle, SingleObserver,
    };

    use super::{Server, ServerEvent, ServerResponse};

    pub struct MockServer {
        observer: MultiObserver<Arc<ServerEvent>>,
        pub implementor: SingleObserver<ServerResponse>,
    }

    impl MockServer {
        pub fn new() -> Self {
            Self {
                observer: MultiObserver::new(),
                implementor: SingleObserver::new(),
            }
        }

        /// Receive FIX messages from clients
        pub fn start_server() {
            todo!()
        }

        /// Notify about FIX messages
        ///
        /// Real server would parse FIX message, this one just publishes the event as-is.
        pub fn notify_server_event(&self, server_event: Arc<ServerEvent>) {
            self.observer.publish_many(&server_event);
        }
    }

    impl Server for MockServer {
        /// provide methods for sending FIX responses
        fn respond_with(&mut self, response: ServerResponse) {
            self.implementor.publish_single(response);
        }
    }

    impl IntoObservableMany<Arc<ServerEvent>> for MockServer {
        fn get_multi_observer_mut(&mut self) -> &mut MultiObserver<Arc<ServerEvent>> {
            &mut self.observer
        }
    }
}
