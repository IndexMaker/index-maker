use crate::core::bits::{Amount, ClientOrderId, Side, Symbol};

pub enum IndexOrderEvent {
    NewIndexOrder {
        symbol: Symbol,
        price: Amount,
        quantity: Amount,
        side: Side,
        client_order_id: ClientOrderId,
    },
    CancelIndexOrder {
        client_order_id: (),
    },
}

/// manage index orders, receive orders and route into solver
pub trait IndexOrderManager {
    /// provide a method to list pending index order requests
    fn get_pending_order_requests(&self) -> Vec<()>;

    /// provide a method to fill index order request
    fn fill_order_request(&mut self, client_order_id: ClientOrderId, fill_amount: Amount);
}

#[cfg(test)]
pub mod test_util {
    use std::{collections::HashMap, sync::Arc};

    use parking_lot::RwLock;

    use crate::{
        core::{
            bits::{Amount, ClientOrderId, Side, Symbol},
            functional::SingleObserver,
        },
        server::server::{Server, ServerEvent},
        solver::index_order::IndexOrder,
    };

    use super::{IndexOrderEvent, IndexOrderManager};

    pub struct MockIndexOrderManager {
        pub observer: SingleObserver<IndexOrderEvent>,
        pub server: Arc<RwLock<dyn Server>>,
        pub index_orders: HashMap<Symbol, IndexOrder>,
    }

    impl MockIndexOrderManager {
        pub fn new(server: Arc<RwLock<dyn Server>>) -> Self {
            Self {
                observer: SingleObserver::new(),
                server,
                index_orders: HashMap::new(),
            }
        }

        pub fn notify_index_order(&self, _index_order: ()) {
            self.observer
                .publish_single(IndexOrderEvent::NewIndexOrder {
                    symbol: Symbol::default(),
                    price: Amount::default(),
                    quantity: Amount::default(),
                    side: Side::Buy,
                    client_order_id: ClientOrderId::default(),
                });
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
                ServerEvent::NewIndexOrder => {
                    self.new_index_order(());
                }
                ServerEvent::CancelIndexOrder => todo!(),
                _ => (),
            }
        }
    }

    impl IndexOrderManager for MockIndexOrderManager {
        /// provide a method to list pending index order requests
        fn get_pending_order_requests(&self) -> Vec<()> {
            todo!()
        }

        /// provide a method to fill index order request
        fn fill_order_request(&mut self, _client_order_id: ClientOrderId, _fill_amount: Amount) {
            todo!()
        }
    }
}
