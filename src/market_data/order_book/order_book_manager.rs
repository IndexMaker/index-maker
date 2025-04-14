use std::collections::HashMap;

use crate::core::bits::{Amount, Symbol};
use eyre::Result;

/// manage order books across markets
pub enum OrderBookEvent {
    PriceLevel {
        symbol: Symbol,
        level: u32,
        price: Amount,
        quantity: Amount,
    },
}

pub trait OrderBookManager {
    /// Get amount of liquidity available within specified threshold
    ///
    /// # Arguemts
    /// * `symbols` - mapping between symbol and price
    ///
    /// # Returns
    /// Mapping between symbol and quantity (liquidity available at price within threshold)
    ///
    fn get_liquidity(
        &self,
        symbols: &HashMap<Symbol, Amount>,
        threshold: Amount,
    ) -> Result<HashMap<Symbol, Amount>>;
}

#[cfg(test)]
pub mod test_util {
    use eyre::Result;
    use parking_lot::RwLock;
    use std::{collections::HashMap, sync::Arc};

    use crate::{
        core::{
            bits::{Amount, PricePointEntry, Side, Symbol},
            functional::SingleObserver,
        },
        market_data::{
            market_data_connector::{MarketDataConnector, MarketDataEvent},
            order_book::order_book::PricePointBook,
        },
    };

    use super::{OrderBookEvent, OrderBookManager};

    pub struct MockOrderBookManager {
        pub observer: SingleObserver<OrderBookEvent>,
        pub market_data_connector: Arc<RwLock<dyn MarketDataConnector>>,
        pub order_books: HashMap<Symbol, PricePointBook>,
    }
    impl MockOrderBookManager {
        pub fn new(market_data_connector: Arc<RwLock<dyn MarketDataConnector>>) -> Self {
            Self {
                observer: SingleObserver::new(),
                market_data_connector,
                order_books: HashMap::new(),
            }
        }

        fn notify_order_book(&self) {
            self.observer.publish_single(OrderBookEvent::PriceLevel {
                symbol: Symbol::default(),
                level: 1,
                price: Amount::default(),
                quantity: Amount::default(),
            });
        }

        /// Update order books
        fn update_order_book(
            &self,
            _symbol: &Symbol,
            _bid_updates: &Vec<PricePointEntry>,
            _ask_updates: &Vec<PricePointEntry>,
        ) {
            // 1. find order book for symbol
            // 2. update order book
            // 3. fire an event that book is updated
            self.notify_order_book();
        }

        /// Receive market data
        pub fn handle_market_data(&mut self, event: &MarketDataEvent) {
            match event {
                MarketDataEvent::FullOrderBook {
                    symbol,
                    bid_updates,
                    ask_updates,
                } => {
                    self.update_order_book(symbol, bid_updates, ask_updates);
                }
                _ => (),
            }
        }
    }
    impl OrderBookManager for MockOrderBookManager {
        fn get_liquidity(
            &self,
            symbols: &HashMap<Symbol, Amount>,
            threshold: Amount,
        ) -> Result<HashMap<Symbol, Amount>> {
            let mut result = HashMap::new();
            // get liquidity for each symbol
            for (symbol, price) in symbols {
                if let Some(order_book) = self.order_books.get(&symbol) {
                    // get liquidity from order book
                    let liquidity = order_book.get_liquidity(Side::Sell, price, threshold)?;
                    result.insert(symbol.clone(), liquidity);
                }
            }
            Ok(result)
        }
    }
}
