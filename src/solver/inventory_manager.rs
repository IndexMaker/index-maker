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
    Fill {
        client_order_id: ClientOrderId,
        symbol: Symbol,
        quantity: Amount,
        price: Amount,
        lot_id: LotId,
    },
}

pub struct InventoryManager {
    pub observer: SingleObserver<InventoryEvent>,
    pub order_tracker: Arc<RwLock<OrderTracker>>,
    pub lots: HashMap<Symbol, VecDeque<Lot>>, // only Long side (we're not margin -> we do not support Short side)
}

impl InventoryManager {
    pub fn new(order_tracker: Arc<RwLock<OrderTracker>>) -> Self {
        Self {
            observer: SingleObserver::new(),
            order_tracker,
            lots: HashMap::new()
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
