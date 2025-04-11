use crate::core::bits::{Amount, Symbol};

pub enum PriceEvent {
    PriceChange{symbol: Symbol, quantity: Amount, price: rust_decimal::Decimal}
}

/// track referece prices for rebalancing, and price estimations
pub trait PriceTracker {}

#[cfg(test)]
pub mod test_util {
    use std::sync::Arc;

    use parking_lot::RwLock;

    use crate::{core::functional::SingleObserver, market_data::market_data_connector::{MarketDataConnector, MarketDataEvent}};

    use super::{PriceEvent, PriceTracker};

    pub struct MockPriceTracker {
        pub observer: SingleObserver<PriceEvent>,
        pub market_data_connector: Arc<RwLock<dyn MarketDataConnector>>,
    }
    impl MockPriceTracker {
        pub fn new(market_data_connector: Arc<RwLock<dyn MarketDataConnector>>) -> Self {
            Self {
                observer: SingleObserver::new(),
                market_data_connector,
            }
        }

        pub fn handle_market_data(&self, _event: &MarketDataEvent) {
            todo!()
        }
    }
    impl PriceTracker for MockPriceTracker {}
}
