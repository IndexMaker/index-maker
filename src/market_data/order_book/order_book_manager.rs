use eyre::{Report, Result};
use std::collections::HashMap;

use crate::{
    core::{
        bits::{Amount, PricePointEntry, Side, Symbol},
        functional::{IntoObservableSingle, PublishSingle, SingleObserver},
    },
    market_data::{market_data_connector::MarketDataEvent, order_book::order_book::PricePointBook},
};

/// manage order books across markets
pub enum OrderBookEvent {
    BookUpdate { symbol: Symbol, sequence_number: u64 },
    UpdateError { symbol: Symbol, sequence_number: u64, error: Report },
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
        side: Side,
        symbols: &HashMap<Symbol, Amount>,
    ) -> Result<HashMap<Symbol, Amount>>;
}

pub struct PricePointBookManager {
    observer: SingleObserver<OrderBookEvent>,
    order_books: HashMap<Symbol, PricePointBook>,
    tolerance: Amount,
}

impl PricePointBookManager {
    pub fn new(tolerance: Amount) -> Self {
        Self {
            observer: SingleObserver::new(),
            order_books: HashMap::new(),
            tolerance,
        }
    }

    pub fn get_order_book(&self, symbol: &Symbol) -> Option<&PricePointBook> {
        self.order_books.get(symbol)
    }

    fn notify_order_book(&self, symbol: &Symbol, sequence_number: u64) {
        self.observer.publish_single(OrderBookEvent::BookUpdate {
            symbol: symbol.clone(),
            sequence_number,
        });
    }

    fn notify_order_book_error(&self, symbol: &Symbol, sequence_number: u64, error: Report) {
        self.observer.publish_single(OrderBookEvent::UpdateError {
            symbol: symbol.clone(),
            sequence_number,
            error,
        });
    }

    /// Update order books
    fn update_order_book(
        &mut self,
        symbol: &Symbol,
        sequence_number: u64,
        is_snapshot: bool,
        bid_updates: &Vec<PricePointEntry>,
        ask_updates: &Vec<PricePointEntry>,
    ) {
        // 1. find order book for symbol
        let book = self
            .order_books
            .entry(symbol.clone())
            .or_insert_with(|| PricePointBook::new(self.tolerance));

        // 2. if it is as snapshot, clear the book completely
        if is_snapshot {
            book.clear();
        }

        // 3. update order book
        if let Err(error) = book.update_entries(sequence_number, bid_updates, ask_updates) {
            // 3. fire event, notifying about error
            self.notify_order_book_error(symbol, sequence_number, error);
        } else {
            // 3. fire an event that book is updated
            self.notify_order_book(symbol, sequence_number);
        }
    }

    /// Receive market data
    pub fn handle_market_data(&mut self, event: &MarketDataEvent) {
        match event {
            MarketDataEvent::OrderBookSnapshot {
                symbol,
                sequence_number,
                bid_updates,
                ask_updates,
            } => {
                self.update_order_book(symbol, *sequence_number, true, bid_updates, ask_updates);
            }
            MarketDataEvent::OrderBookDelta {
                symbol,
                sequence_number,
                bid_updates,
                ask_updates,
            } => {
                self.update_order_book(symbol, *sequence_number, false, bid_updates, ask_updates);
            }
            _ => (),
        }
    }
}

impl OrderBookManager for PricePointBookManager {
    fn get_liquidity(
        &self,
        side: Side,
        symbols: &HashMap<Symbol, Amount>,
    ) -> Result<HashMap<Symbol, Amount>> {
        let mut result = HashMap::new();

        // get liquidity for each symbol
        for (symbol, price) in symbols {
            if let Some(order_book) = self.order_books.get(&symbol) {
                // get liquidity from order book
                let liquidity = order_book.get_liquidity(side, price)?;
                result.insert(symbol.clone(), liquidity);
            }
        }

        Ok(result)
    }
}

impl IntoObservableSingle<OrderBookEvent> for PricePointBookManager {
    fn get_single_observer_mut(&mut self) -> &mut SingleObserver<OrderBookEvent> {
        &mut self.observer
    }
}
