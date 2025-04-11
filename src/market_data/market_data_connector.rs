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
pub trait MarketDataConnector {}

#[cfg(test)]
pub mod test_util {

    use std::sync::Arc;

    use crate::core::functional::MultiObserver;

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
    }
    impl MarketDataConnector for MockMarketDataConnector {}
}
