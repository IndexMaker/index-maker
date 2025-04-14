use std::collections::HashMap;

use crate::core::bits::{Amount, ClientOrderId, LotId, Symbol};

pub enum InventoryEvent {
    Fill {
        client_order_id: ClientOrderId,
        symbol: Symbol,
        quantity: Amount,
        price: Amount,
        lot_id: LotId,
    },
}

pub trait InventoryManager {
    /// receive new order requests from Solver
    fn new_order(&self, index_order: ());

    /// provide method to get open lots
    fn get_open_lots(&self, symbols: &[Symbol]) -> HashMap<Symbol, Amount>;
}

#[cfg(test)]
pub mod test_util {
    use std::{
        collections::{HashMap, VecDeque},
        sync::Arc,
    };

    use parking_lot::RwLock;

    use crate::{
        core::{
            bits::{Amount, ClientOrderId, Lot, LotId, Symbol},
            functional::SingleObserver,
        },
        order_sender::order_tracker::{OrderTracker, OrderTrackerNotification},
    };

    use super::{InventoryEvent, InventoryManager};

    pub struct MockInventoryManager {
        pub observer: SingleObserver<InventoryEvent>,
        pub order_tracker: Arc<RwLock<dyn OrderTracker>>,
        pub lots: VecDeque<Lot>, // only Long side (we're not margin -> we do not support Short side)
    }

    impl MockInventoryManager {
        pub fn new(order_tracker: Arc<RwLock<dyn OrderTracker>>) -> Self {
            Self {
                observer: SingleObserver::new(),
                order_tracker,
                lots: VecDeque::default(),
            }
        }

        /// notify about new lots to subscriber (-> Solver)
        pub fn notify_lots(&self, _lots: &[Lot]) {
            // fire an event to the subscriber (-> Solver)
            self.observer.publish_single(InventoryEvent::Fill {
                client_order_id: ClientOrderId::default(),
                symbol: Symbol::default(),
                quantity: Amount::default(),
                price: Amount::default(),
                lot_id: LotId::default(),
            });
        }

        /// provide method to allocate lots to individual index orders
        pub fn allocate_lots(&self, _index_order: ()) {
            // notify subscriber (->Solver) about new lots
            self.notify_lots(&[Lot::default()]);
        }

        /// receive fill reports from Order Tracker
        pub fn handle_fill_report(&self, _report: OrderTrackerNotification) {
            // 1. match against lots (in case of Sell), P&L report
            // 2. allocate new lots, store Cost/Fees
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

        /// provide method to get open lots
        fn get_open_lots(&self, _symbols: &[Symbol]) -> HashMap<Symbol, Amount> {
            todo!()
        }
    }
}
