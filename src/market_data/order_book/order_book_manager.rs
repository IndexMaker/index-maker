use crate::core::bits::{Amount, Symbol};

/// manage order books across markets
pub enum OrderBookEvent {
    PriceLevel{symbol: Symbol, level: u32, price: Amount, quantity: Amount}
}

pub trait OrderBookManager {}

#[cfg(test)]
pub mod test_util {
    use std::sync::Arc;

    use parking_lot::RwLock;

    use crate::{core::functional::SingleObserver, market_data::market_data_connector::{MarketDataConnector, MarketDataEvent}};

    use super::{OrderBookEvent, OrderBookManager};

    pub struct MockOrderBookManager {
        pub observer: SingleObserver<OrderBookEvent>,
        pub market_data_connector: Arc<RwLock<dyn MarketDataConnector>>,
    }
    impl MockOrderBookManager {
        pub fn new(market_data_connector: Arc<RwLock<dyn MarketDataConnector>>) -> Self {
            Self {
                observer: SingleObserver::new(),
                market_data_connector,
            }
        }

        pub fn handle_market_data(&self, _event: &MarketDataEvent) {}
    }
    impl OrderBookManager for MockOrderBookManager {}
}
