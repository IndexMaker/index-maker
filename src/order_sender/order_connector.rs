use crate::core::bits::{Amount, OrderId, Symbol};

/// abstract, allow sending orders and cancels, receiving acks, naks, executions
pub enum OrderConnectorNotification {
    Fill {
        symbol: Symbol,
        order_id: OrderId,
        price: Amount,
        quantity: Amount,
    },
    Cancel {
        symbol: Symbol,
        order_id: OrderId,
        quantity: Amount,
    },
}

pub trait OrderConnector {
    /// Send orders
    fn send_order(&mut self, order: ());
}

#[cfg(test)]
pub mod test_util {
    use crate::core::{
        bits::{Amount, OrderId, Symbol},
        functional::SingleObserver,
    };

    use super::{OrderConnector, OrderConnectorNotification};

    pub struct MockOrderConnector {
        pub observer: SingleObserver<OrderConnectorNotification>,
    }

    impl MockOrderConnector {
        pub fn new() -> Self {
            Self {
                observer: SingleObserver::new(),
            }
        }

        /// Receive fills from exchange, and publish an event to subscrber (-> Order Tracker)
        pub fn notify_fill(&self, _fill: ()) {
            self.observer
                .publish_single(OrderConnectorNotification::Fill {
                    symbol: Symbol::default(),
                    order_id: OrderId::default(),
                    price: Amount::default(),
                    quantity: Amount::default(),
                });
        }

        /// Recive cancel from exchange and publish an event to subscrber (-> Order Tracker)
        pub fn notify_cancel(&self, _cancel: ()) {
            self.observer
                .publish_single(OrderConnectorNotification::Cancel {
                    symbol: Symbol::default(),
                    order_id: OrderId::default(),
                    quantity: Amount::default(),
                });
        }

        /// Connect to exchange
        pub fn connect(&mut self) {
            // connect to exchange (-> Binance)
        }
    }

    impl OrderConnector for MockOrderConnector {
        /// Send orders
        fn send_order(&mut self, _order: ()) {
            // send order to exchange (-> Binance)
        }
    }
}
