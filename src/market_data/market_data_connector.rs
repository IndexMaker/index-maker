use crate::core::bits::{Amount, Symbol};

/// abstract, connect to receive market data (live or mock)
pub enum MarketDataEvent {
    TopOfBook {
        symbol: Symbol,
        bid: Amount,
        ask: Amount,
        bid_quantity: Amount,
        ask_quantity: Amount,
    },
    Trade {
        symbol: Symbol,
        price: Amount,
        quantity: Amount,
    },
    FullOrderBook {
        symbol: Symbol,
        price_levels: (),
    },
}
pub trait MarketDataConnector {
    /// Subscribe to set of symbols
    fn subscribe(&self, symbols: &[Symbol]);
}

#[cfg(test)]
pub mod test_util {

    use std::sync::Arc;

    use crate::core::{
        bits::{Amount, Symbol},
        functional::MultiObserver,
    };

    use super::{MarketDataConnector, MarketDataEvent};

    pub struct MockMarketDataConnector {
        pub observer: MultiObserver<Arc<MarketDataEvent>>,
    }

    impl MockMarketDataConnector {
        pub fn new() -> Self {
            Self {
                observer: MultiObserver::new(),
            }
        }

        /// receive market data from exchange (-> PriceTracker)
        pub fn notify_top_of_book(&self, _market_data: ()) {
            self.observer
                .publish_many(&Arc::new(MarketDataEvent::TopOfBook {
                    symbol: Symbol::default(),
                    bid: Amount::default(),
                    ask: Amount::default(),
                    bid_quantity: Amount::default(),
                    ask_quantity: Amount::default(),
                }));
        }

        /// receive market data from exchange (-> PriceTracker)
        pub fn notify_trade(&self, _trade: ()) {
            self.observer
                .publish_many(&Arc::new(MarketDataEvent::Trade {
                    symbol: Symbol::default(),
                    price: Amount::default(),
                    quantity: Amount::default(),
                }));
        }

        /// receive market data from exchange (-> OrderBookManager)
        pub fn notify_full_order_book(&self, _book: ()) {
            self.observer
                .publish_many(&Arc::new(MarketDataEvent::FullOrderBook {
                    symbol: Symbol::default(),
                    price_levels: (),
                }));
        }

        /// Connect to exchange (-> Binance)
        pub fn connect(&mut self) {}
    }

    impl MarketDataConnector for MockMarketDataConnector {
        /// Subscribe to set of symbols
        fn subscribe(&self, _symbols: &[Symbol]) {
        }
    }
}
