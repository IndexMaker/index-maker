use std::{
    collections::{HashMap, VecDeque},
    sync::Arc,
};

use parking_lot::RwLock;

use crate::{
    core::{
        bits::{Amount, ClientOrderId, Lot, LotId, Order, OrderId, Side, Symbol},
        functional::SingleObserver,
    },
    order_sender::order_tracker::{OrderTracker, OrderTrackerNotification},
};

pub enum InventoryEvent {
    OpenLot {
        order_id: OrderId,
        client_order_id: ClientOrderId,
        lot_id: LotId,
        symbol: Symbol,
        price: Amount,
        quantity: Amount,
    },
    CloseLot {
        original_order_id: OrderId,
        original_client_order_id: ClientOrderId,
        original_lot_id: LotId,
        closing_order_id: OrderId,
        closing_client_order_id: ClientOrderId,
        closing_lot_id: LotId,
        symbol: Symbol,
        original_price: Amount,     // original price when lot was opened
        closing_price: Amount,      // price in this closing event
        quantity_closed: Amount,    // quantity closed in this event
        original_quantity: Amount,  // original quantity when lot was opened
        quantity_remaining: Amount, // quantity remaining in the lot
    },
}

pub struct InventoryManager {
    pub observer: SingleObserver<InventoryEvent>,
    pub order_tracker: Arc<RwLock<OrderTracker>>,
    pub lots: HashMap<Symbol, VecDeque<Arc<RwLock<Lot>>>>,
}

impl InventoryManager {
    pub fn new(order_tracker: Arc<RwLock<OrderTracker>>) -> Self {
        Self {
            observer: SingleObserver::new(),
            order_tracker,
            lots: HashMap::new(),
        }
    }

    /// notify about new lots to subscriber (-> Solver)
    pub fn notify_lots(&self, _lots: &[Lot]) {
        // fire an event to the subscriber (-> Solver)
        //self.observer.publish_single(InventoryEvent::OpenLot {
        //    order_id: (),
        //    client_order_id: (),
        //    lot_id: (),
        //    symbol: (),
        //    price: (),
        //    quantity: (),
        //});
    }

    /// provide method to allocate lots to individual index orders
    pub fn allocate_lots(&self, _index_order: ()) {
        // notify subscriber (->Solver) about new lots
        self.notify_lots(&[Lot::default()]);
    }

    /// receive fill reports from Order Tracker
    pub fn handle_fill_report(&self, notification: OrderTrackerNotification) {
        // 1. match against lots (in case of Sell), P&L report
        // 2. allocate new lots, store Cost/Fees
        //self.notify_lots(&[Lot::default()]);
        match notification {
            OrderTrackerNotification::Fill {
                order_id: _,
                client_order_id: _,
                lot_id: _,
                symbol: _,
                side,
                price_filled: _,
                quantity_filled: _,
                original_quantity: _,
                quantity_remaining: _,
            } => {
                // open or close lot (lot matching)
                match side {
                    Side::Buy => {
                        // open new lot
                        // send OpenLot event to subscriber (-> Solver)
                    }
                    Side::Sell => {
                        // match against open lots, close lots
                        // send CloseLot event to subscriber (-> Solver)
                    }
                }
            }
            OrderTrackerNotification::Cancel {
                order_id: _,
                client_order_id: _,
                symbol: _,
                side: _,
                quantity_cancelled: _,
                original_quantity: _,
                quantity_remaining: _,
            } => {
                // TBD: Cancel doesn't open onor close any lots. It's just a
                // notification to subscriber that order was cancelled
            }
        }
    }

    /// receive new order requests from Solver
    pub fn new_order(&self, index_order: ()) {
        // 1. match the lots
        // 2. notify if there is any matching lots
        self.allocate_lots(index_order);
        // 3. for remaining quantity send new order requests to order tracker
        self.order_tracker
            .write()
            .new_order(Arc::new(Order {
                order_id: OrderId::default(),
                client_order_id: ClientOrderId::default(),
                symbol: Symbol::default(),
                side: Side::Buy,
                price: Amount::ZERO,
                quantity: Amount::ZERO,
            }))
            .unwrap();
    }

    /// provide method to get open lots
    pub fn get_open_lots(&self, _symbols: &[Symbol]) -> HashMap<Symbol, Amount> {
        todo!()
    }
}

#[cfg(test)]
mod test {
    #[test]
    fn test_inventory_manager() {}
}
