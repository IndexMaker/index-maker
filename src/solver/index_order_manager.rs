use std::{collections::HashMap, sync::Arc};

use chrono::{DateTime, Utc};
use parking_lot::RwLock;

use crate::{
    core::functional::SingleObserver,
    server::server::{Server, ServerEvent},
    solver::index_order::IndexOrder,
};

use crate::core::bits::{Address, Amount, BatchOrderId, ClientOrderId, Side, Symbol};

pub enum IndexOrderEvent {
    NewIndexOrder {
        address: Address,
        index_order_id: ClientOrderId,
        symbol: Symbol,
        side: Side,
        price: Amount,
        price_threshold: Amount,
        quantity: Amount,
        timestamp: DateTime<Utc>,
    },
    CancelIndexOrder {
        address: Address,
        index_order_id: ClientOrderId,
        quantity: Amount,
    },
}

pub struct IndexOrderManager {
    pub observer: SingleObserver<IndexOrderEvent>,
    pub server: Arc<RwLock<dyn Server>>,
    pub index_orders: HashMap<Address, HashMap<Symbol, Arc<RwLock<IndexOrder>>>>,
}

/// manage index orders, receive orders and route into solver
impl IndexOrderManager {
    pub fn new(server: Arc<RwLock<dyn Server>>) -> Self {
        Self {
            observer: SingleObserver::new(),
            server,
            index_orders: HashMap::new(),
        }
    }

    pub fn notify_index_order(&self, _index_order: ()) {
        //self.observer
        //    .publish_single(IndexOrderEvent::NewIndexOrder {
        //        symbol: Symbol::default(),
        //        price: Amount::default(),
        //        quantity: Amount::default(),
        //        side: Side::Buy,
        //        client_order_id: BatchOrderId::default(),
        //    });
    }

    fn new_index_order(&mut self, _order: ()) {
        // 1. compact order request with existing orders
        // self.index_orders.entry(key)
        // 2. notify subscriber (-> Solver)
        self.notify_index_order(());
    }

    /// receive index order requests from (FIX) Server
    pub fn handle_server_message(&mut self, notification: &ServerEvent) {
        match notification {
            ServerEvent::NewIndexOrder {
                address: _,
                client_order_id: _,
                symbol: _,
                side: _,
                price: _,
                price_threshold: _,
                quantity: _,
                timestamp: _,
            } => {
                self.new_index_order(());
            }
            ServerEvent::CancelIndexOrder {
                address: _,
                client_order_id: _,
                symbol: _,
                quantity: _,
                timestamp: _,
            } => todo!(),
            _ => (),
        }
    }

    /// provide a method to fill index order request
    pub fn fill_order_request(&mut self, _client_order_id: BatchOrderId, _fill_amount: Amount) {
        todo!()
    }

    /// provide a method to list pending index order requests
    pub fn get_pending_order_requests(&self) -> Vec<()> {
        todo!()
    }
}
