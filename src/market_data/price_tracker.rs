use itertools::partition;
use std::{cmp::max, collections::HashMap};

use crate::{
    core::{
        bits::{Amount, LastPriceEntry, PriceType, Symbol},
        functional::{IntoObservableSingle, PublishSingle, SingleObserver},
    },
    market_data::market_data_connector::MarketDataEvent,
};

pub enum PriceEvent {
    PriceChange {
        symbol: Symbol,
        sequence_number: u64,
    },
    Trade {
        symbol: Symbol,
        sequence_number: u64,
    },
}

pub struct GetPricesResponse {
    pub prices: HashMap<Symbol, Amount>,
    pub missing_symbols: Vec<Symbol>,
}

/// track referece prices for rebalancing, and price estimations
pub struct PriceTracker {
    observer: SingleObserver<PriceEvent>,
    pub prices: HashMap<Symbol, LastPriceEntry>,
}

impl PriceTracker {
    pub fn new() -> Self {
        Self {
            observer: SingleObserver::new(),
            prices: HashMap::new(),
        }
    }

    fn notify_price_changed(&self, symbol: &Symbol, sequence_number: u64) {
        self.observer.publish_single(PriceEvent::PriceChange {
            symbol: symbol.clone(),
            sequence_number,
        });
    }

    fn notify_trade(&self, symbol: &Symbol, sequence_number: u64) {
        self.observer.publish_single(PriceEvent::Trade {
            symbol: symbol.clone(),
            sequence_number,
        });
    }

    /// Calculate prices
    pub fn update_top(
        &mut self,
        symbol: &Symbol,
        sequence_number: u64,
        best_bid_price: &Amount,
        best_ask_price: &Amount,
        best_bid_quantity: &Amount,
        best_ask_quantity: &Amount,
    ) {
        self.prices
            .entry(symbol.clone())
            .and_modify(|value| {
                value.sequence_number = max(sequence_number, value.sequence_number);
                value.best_bid_price = Some(*best_bid_price);
                value.best_ask_price = Some(*best_ask_price);
                value.best_bid_quantity = *best_bid_quantity;
                value.best_ask_quantity = *best_ask_quantity;
            })
            .or_insert_with(|| LastPriceEntry {
                sequence_number,
                best_bid_price: Some(*best_bid_price),
                best_ask_price: Some(*best_ask_price),
                best_bid_quantity: *best_bid_quantity,
                best_ask_quantity: *best_ask_quantity,
                last_trade_price: None,
                last_trade_quantity: Amount::ZERO,
            });

        // Notify observer that price for symbol changed. Observer will then decide which type of prices to ask for.
        self.notify_price_changed(symbol, sequence_number);
    }

    pub fn update_last_trade(
        &mut self,
        symbol: &Symbol,
        sequence_number: u64,
        price: &Amount,
        quantity: &Amount,
    ) {
        self.prices
            .entry(symbol.clone())
            .and_modify(|value| {
                value.sequence_number = max(sequence_number, value.sequence_number);
                value.last_trade_price = Some(*price);
                value.last_trade_quantity = *quantity;
            })
            .or_insert_with(|| LastPriceEntry {
                sequence_number,
                best_bid_price: None,
                best_ask_price: None,
                best_bid_quantity: Amount::ZERO,
                best_ask_quantity: Amount::ZERO,
                last_trade_price: Some(*price),
                last_trade_quantity: *quantity,
            });

        // Notify observer that price for symbol changed. Observer will then decide which type of prices to ask for.
        self.notify_trade(symbol, sequence_number);
    }

    /// Receive market data
    pub fn handle_market_data(&mut self, event: &MarketDataEvent) {
        match event {
            MarketDataEvent::TopOfBook {
                symbol,
                sequence_number,
                best_bid_price,
                best_ask_price,
                best_bid_quantity,
                best_ask_quantity,
            } => {
                self.update_top(
                    symbol,
                    *sequence_number,
                    best_bid_price,
                    best_ask_price,
                    best_bid_quantity,
                    best_ask_quantity,
                );
            }
            MarketDataEvent::Trade {
                symbol,
                sequence_number,
                price,
                quantity,
            } => {
                self.update_last_trade(symbol, *sequence_number, price, quantity);
            }
            _ => (),
        }
    }

    /// Provide method of retrieving prices
    pub fn get_prices(&self, price_type: PriceType, symbols: &[Symbol]) -> GetPricesResponse {
        // Optimistic: we should be able to find all symbols
        let mut prices = HashMap::with_capacity(symbols.len());

        // ...but first we copy all symbols into a Vec
        let mut missing_symbols = symbols.to_vec();

        // ...and then we should collect last prices, and find the missing ones
        //
        // FYI: This is equivalent of C++ std::remove_if(), which rearranges elements
        // so that elements matching the predicate are all before the partition point.
        let partition_point = partition(&mut missing_symbols, |symbol| {
            if let Some(price) = self.prices.get(symbol) {
                if let Some(price) = price.get_price(price_type) {
                    prices.insert(symbol.clone(), price);
                    false
                } else {
                    // price is missing
                    true
                }
            } else {
                true
            }
        });

        // FYI: This is equivalent of C++ std::vector::erase()
        // so that all elements after partition_point are removed
        missing_symbols.splice(partition_point.., []);

        // move prices and missing into const response
        GetPricesResponse {
            prices,
            missing_symbols,
        }
    }
}

impl IntoObservableSingle<PriceEvent> for PriceTracker {
    fn get_single_observer_mut(&mut self) -> &mut SingleObserver<PriceEvent> {
        &mut self.observer
    }
}

#[cfg(test)]
mod test {
    use std::collections::HashMap;

    use rust_decimal::dec;

    use crate::{
        assert_hashmap_amounts_eq,
        core::{
            bits::{Amount, PriceType, Symbol},
            test_util::{
                flag_mock_atomic_bool, get_mock_asset_name_1, get_mock_asset_name_2,
                get_mock_atomic_bool_pair, reset_flag_mock_atomic_bool, test_mock_atomic_bool,
            },
        },
        market_data::market_data_connector::MarketDataEvent,
    };

    use super::{PriceEvent, PriceTracker};

    #[test]
    fn test_price_tracker() {
        let mut price_tracker = PriceTracker::new();

        let symbols = [get_mock_asset_name_1(), get_mock_asset_name_2()];

        let prices = price_tracker.get_prices(PriceType::BestAsk, &symbols);

        assert_eq!(prices.prices, HashMap::new());
        assert_eq!(prices.missing_symbols, symbols);

        let (called_notify, called_notify_inner) = get_mock_atomic_bool_pair();

        price_tracker.observer.set_observer_fn(move |e| {
            match e {
                PriceEvent::PriceChange {
                    symbol,
                    sequence_number,
                } => {
                    flag_mock_atomic_bool(&called_notify_inner);
                    assert_eq!(symbol, get_mock_asset_name_1());
                    assert_eq!(sequence_number, 1);
                },
                PriceEvent::Trade { symbol, sequence_number } => {
                    assert!(false);
                }
            };
        });

        price_tracker.handle_market_data(&MarketDataEvent::TopOfBook {
            symbol: get_mock_asset_name_1(),
            sequence_number: 1,
            best_bid_price: dec!(12500.00),
            best_ask_price: dec!(12500.50),
            best_bid_quantity: dec!(1500.00),
            best_ask_quantity: dec!(1000.00),
        });

        assert!(test_mock_atomic_bool(&called_notify));

        let tolerance = dec!(0.0001);

        let prices = price_tracker.get_prices(PriceType::BestBid, &symbols);

        assert_eq!(prices.missing_symbols, [get_mock_asset_name_2()]);
        assert_hashmap_amounts_eq!(
            prices.prices,
            [(get_mock_asset_name_1(), dec!(12500.00))].into(),
            tolerance
        );

        let prices = price_tracker.get_prices(PriceType::BestAsk, &symbols);

        assert_eq!(prices.missing_symbols, [get_mock_asset_name_2()]);
        assert_hashmap_amounts_eq!(
            prices.prices,
            [(get_mock_asset_name_1(), dec!(12500.50))].into(),
            tolerance
        );

        let prices = price_tracker.get_prices(PriceType::MidPoint, &symbols);

        assert_eq!(prices.missing_symbols, [get_mock_asset_name_2()]);
        assert_hashmap_amounts_eq!(
            prices.prices,
            [(get_mock_asset_name_1(), dec!(12500.25))].into(),
            tolerance
        );

        let prices = price_tracker.get_prices(PriceType::VolumeWeighted, &symbols);

        assert_eq!(prices.missing_symbols, [get_mock_asset_name_2()]);
        assert_hashmap_amounts_eq!(
            prices.prices,
            [(get_mock_asset_name_1(), dec!(12500.20))].into(),
            tolerance
        );

        let called_notify_inner = called_notify.clone();

        price_tracker.observer.set_observer_fn(move |e| {
            match e {
                PriceEvent::PriceChange {
                    symbol,
                    sequence_number,
                } => {
                    flag_mock_atomic_bool(&called_notify_inner);
                    assert_eq!(symbol, get_mock_asset_name_2());
                    assert_eq!(sequence_number, 2);
                },
                PriceEvent::Trade { symbol, sequence_number } => {
                    assert!(false);
                }
            };
        });

        reset_flag_mock_atomic_bool(&called_notify);
        assert!(!test_mock_atomic_bool(&called_notify));

        price_tracker.handle_market_data(&MarketDataEvent::Trade {
            symbol: get_mock_asset_name_2(),
            sequence_number: 2,
            price: dec!(700.50),
            quantity: dec!(400.00),
        });

        assert!(test_mock_atomic_bool(&called_notify));

        let prices = price_tracker.get_prices(PriceType::LastTrade, &symbols);

        assert_eq!(prices.missing_symbols, [get_mock_asset_name_1()]);
        assert_hashmap_amounts_eq!(
            prices.prices,
            [(get_mock_asset_name_2(), dec!(700.50))].into(),
            tolerance
        );
    }
}
