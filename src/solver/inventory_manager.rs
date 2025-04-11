use crate::core::bits::Amount;

pub enum InventoryEvent {
    Fill {
        client_order_id: (),
        quantity: Amount,
        price: Amount,
    },
}

pub trait InventoryManager {}

#[cfg(test)]
pub mod test_util {
    use std::sync::Arc;

    use crate::{core::functional::SingleObserver, order_sender::order_tracker::OrderTracker};

    use super::{InventoryEvent, InventoryManager};

    pub struct MockInventoryManager {
        pub observer: SingleObserver<InventoryEvent>,
        pub order_tracker: Arc<dyn OrderTracker>,
    }

    impl MockInventoryManager {
        pub fn new(order_tracker: Arc<dyn OrderTracker>) -> Self {
            Self {
                observer: SingleObserver::new(),
                order_tracker,
            }
        }
    }

    impl InventoryManager for MockInventoryManager {}
}
