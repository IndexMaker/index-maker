/// track orders that we sent to
pub trait OrderTracker {}

#[cfg(test)]
pub mod test_util {
    use std::sync::Arc;

    use crate::order_sender::order_connector::OrderConnector;

    use super::OrderTracker;

    pub struct MockOrderTracker {
        pub order_connector: Arc<dyn OrderConnector>,
    }
    impl MockOrderTracker {
        pub fn new(order_connector: Arc<dyn OrderConnector>) -> Self {
            Self { order_connector }
        }
    }
    impl OrderTracker for MockOrderTracker {}
}
