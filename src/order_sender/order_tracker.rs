use crate::core::bits::{Amount, OrderId};

/// track orders that we sent to
pub enum OrderTrackerNotification {
    Fill {
        order_id: OrderId,
        quantity: Amount,
        price: Amount,
    },
}

pub trait OrderTracker {
    /// Receive new order requests from InventoryManager
    fn new_order(&mut self, order: ());
}

#[cfg(test)]
pub mod test_util {
    use std::{collections::HashMap, sync::Arc};

    use parking_lot::RwLock;

    use crate::{
        core::{
            bits::{Amount, ClientOrderId, Order, OrderId},
            functional::SingleObserver,
        },
        order_sender::order_connector::{OrderConnector, OrderConnectorNotification},
    };

    use super::{OrderTracker, OrderTrackerNotification};

    pub struct MockOrderTracker {
        pub observer: SingleObserver<OrderTrackerNotification>,
        pub order_connector: Arc<RwLock<dyn OrderConnector>>,
        pub orders: HashMap<ClientOrderId, Order>,
    }

    impl MockOrderTracker {
        pub fn new(order_connector: Arc<RwLock<dyn OrderConnector>>) -> Self {
            Self {
                observer: SingleObserver::new(),
                order_connector,
                orders: HashMap::new(),
            }
        }

        /// Notify about fills sending notification to subscriber (-> Inventory Manager)
        pub fn notify_fill(&self, _fill: ()) {
            self.observer
                .publish_single(OrderTrackerNotification::Fill {
                    order_id: OrderId::default(),
                    quantity: Amount::ZERO,
                    price: Amount::ZERO,
                });
        }

        /// Receive execution reports from OrderConnector
        pub fn handle_order_notification(&self, _execution_report: OrderConnectorNotification) {
            // 1. do book-keeping of orders
            // 2. send notification to subscriber (-> Inventory Manager)
            self.notify_fill(());
        }
    }

    impl OrderTracker for MockOrderTracker {
        /// Receive new order requests from InventoryManager
        fn new_order(&mut self, _order: ()) {
            //self.orders.entry().or_insert();
            self.order_connector.write().send_order(());
        }
    }
}
