use crate::core::bits::{Amount, Symbol};

pub enum IndexOrderEvent {
    NewIndexOrder {
        symbol: Symbol,
        price: Amount,
        quantity: Amount,
        side: (),
        client_order_id: (),
    },
    CancelIndexOrder {
        client_order_id: (),
    },
}

/// manage index orders, receive orders and route into solver
pub trait IndexOrderManager {
    //..more
}

#[cfg(test)]
pub mod test_util {
    use std::sync::Arc;

    use parking_lot::RwLock;

    use crate::{
        core::functional::SingleObserver,
        server::server::{Server, ServerEvent},
    };

    use super::{IndexOrderEvent, IndexOrderManager};

    pub struct MockIndexOrderManager {
        pub observer: SingleObserver<IndexOrderEvent>,
        pub server: Arc<RwLock<dyn Server>>,
    }

    impl MockIndexOrderManager {
        pub fn new(server: Arc<RwLock<dyn Server>>) -> Self {
            Self {
                observer: SingleObserver::new(),
                server,
            }
        }

        pub fn handle_server_message(&self, _notification: &ServerEvent) {
            todo!()
        }
    }

    impl IndexOrderManager for MockIndexOrderManager {}
}
