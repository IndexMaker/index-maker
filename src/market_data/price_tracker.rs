use std::collections::HashMap;

use crate::core::bits::{Amount, Symbol};

pub enum PriceEvent {
    PriceChange {
        symbol: Symbol,
        quantity: Amount,
        price: rust_decimal::Decimal,
    },
}

/// track referece prices for rebalancing, and price estimations
pub trait PriceTracker {
    /// Provide method of retrieving prices
    fn get_prices(&self, symbols: &[Symbol]) -> HashMap<Symbol, Amount>;
}

#[cfg(test)]
pub mod test_util {
    use std::{collections::HashMap, sync::Arc};

    use parking_lot::RwLock;

    use crate::{
        core::{
            bits::{Amount, Symbol},
            functional::SingleObserver,
        },
        market_data::market_data_connector::{MarketDataConnector, MarketDataEvent},
    };

    use super::{PriceEvent, PriceTracker};

    pub struct MockPriceTracker {
        pub observer: SingleObserver<PriceEvent>,
        pub market_data_connector: Arc<RwLock<dyn MarketDataConnector>>,
        pub prices: HashMap<Symbol, Amount>,
    }

    impl MockPriceTracker {
        pub fn new(market_data_connector: Arc<RwLock<dyn MarketDataConnector>>) -> Self {
            Self {
                observer: SingleObserver::new(),
                market_data_connector,
                prices: HashMap::new(),
            }
        }

        /// Calculate prices
        pub fn calculate_prices(
            &mut self,
            _symbol: &Symbol,
            _bid: &Amount,
            _ask: &Amount,
            _bid_quantity: &Amount,
            _ask_quantity: &Amount,
        ) {
            todo!()
        }

        pub fn update_last_trade(&mut self, _symbol: &Symbol, _price: &Amount, _quantity: &Amount) {
            todo!()
        }

        /// Receive market data
        pub fn handle_market_data(&mut self, event: &MarketDataEvent) {
            match event {
                MarketDataEvent::TopOfBook {
                    symbol,
                    bid,
                    ask,
                    bid_quantity,
                    ask_quantity,
                } => {
                    self.calculate_prices(symbol, bid, ask, bid_quantity, ask_quantity);
                }
                MarketDataEvent::Trade {
                    symbol,
                    price,
                    quantity,
                } => {
                    self.update_last_trade(symbol, price, quantity);
                }
                _ => (),
            }
        }
    }

    impl PriceTracker for MockPriceTracker {
        /// Provide method of retrieving prices
        fn get_prices(&self, _symbols: &[Symbol]) -> HashMap<Symbol, Amount> {
            todo!()
        }
    }
}
