pub enum ServerEvent {
    NewIndexOrder,
    NewQuoteRequest,
    CancelIndexOrder,
    CancelQuoteRequest,
    AccountToCustody,
    CustodyToAccount,
}

pub trait Server {}

#[cfg(test)]
pub mod test_util {

    use std::sync::Arc;

    use crate::core::functional::MultiObserver;

    use super::{Server, ServerEvent};

    pub struct MockServer {
        pub observers: MultiObserver<Arc<ServerEvent>>,
    }

    impl MockServer {
        pub fn new() -> Self {
            Self {
                observers: MultiObserver::new(),
            }
        }
    }

    impl Server for MockServer {}
}
