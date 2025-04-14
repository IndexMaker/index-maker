use crate::core::bits::{Amount, ClientOrderId, LotId};

pub enum InventoryEvent {
    Fill {
        client_order_id: ClientOrderId,
        quantity: Amount,
        price: Amount,
        lot_id: LotId
    },
}

pub trait InventoryManager {
    /// receive new order requests from Solver 
    fn new_order(&self, index_order: ());
}

#[cfg(test)]
pub mod test_util {
    use std::{collections::VecDeque, sync::Arc};

    use parking_lot::RwLock;

    use crate::{core::{bits::{Amount, ClientOrderId, Lot, LotId}, functional::SingleObserver}, order_sender::order_tracker::{OrderTracker, OrderTrackerNotification}};

    use super::{InventoryEvent, InventoryManager};

    pub struct MockInventoryManager {
        pub observer: SingleObserver<InventoryEvent>,
        pub order_tracker: Arc<RwLock<dyn OrderTracker>>,
        pub lots: VecDeque<Lot> // only Long side (we're not margin -> we do not support Short side)
    }

    impl MockInventoryManager {
        pub fn new(order_tracker: Arc<RwLock<dyn OrderTracker>>) -> Self {
            Self {
                observer: SingleObserver::new(),
                order_tracker,
                lots: VecDeque::default()
            }
        }

        /// notify about new lots to subscriber (-> Solver)
        pub fn notify_lots(&self, _lots: &[Lot]) {
            // fire an event to the subscriber (-> Solver) 
            self.observer.publish_single(InventoryEvent::Fill {
                client_order_id: ClientOrderId::default(),
                quantity: Amount::ZERO,
                price: Amount::ZERO,
                lot_id: LotId::default()
            });
        }

        /// provide method to allocate lots to individual index orders
        pub fn allocate_lots(&self, _index_order: ()) {
            // notify subscriber (->Solver) about new lots
            self.notify_lots(&[Lot::default()]);
        }

        /// receive fill reports from Order Tracker
        pub fn handle_fill_report(&self, _report: OrderTrackerNotification) {
            // 1. match against lots
            // 2. allocate new lots
            self.notify_lots(&[Lot::default()]);
        }
    }

    impl InventoryManager for MockInventoryManager {
        /// receive new order requests from Solver 
        fn new_order(&self, index_order: ()) {
            // 1. match the lots
            // 2. notify if there is any matching lots
            self.allocate_lots(index_order);
            // 3. for remaining quantity send new order requests to order tracker
            self.order_tracker.write().new_order(());
        }
    }
}
