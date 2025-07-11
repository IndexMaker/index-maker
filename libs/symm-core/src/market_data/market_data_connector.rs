use crate::core::bits::{Amount, PricePointEntry, Symbol};
use eyre::Result;

/// abstract, connect to receive market data (live or mock)
pub enum MarketDataEvent {
    TopOfBook {
        symbol: Symbol,
        sequence_number: u64,
        best_bid_price: Amount,
        best_ask_price: Amount,
        best_bid_quantity: Amount,
        best_ask_quantity: Amount,
    },
    Trade {
        symbol: Symbol,
        sequence_number: u64,
        price: Amount,
        quantity: Amount,
    },
    OrderBookSnapshot {
        symbol: Symbol,
        sequence_number: u64,
        bid_updates: Vec<PricePointEntry>,
        ask_updates: Vec<PricePointEntry>,
    },
    OrderBookDelta {
        symbol: Symbol,
        sequence_number: u64,
        bid_updates: Vec<PricePointEntry>,
        ask_updates: Vec<PricePointEntry>,
    },
}

#[derive(Clone, Hash, PartialEq, Eq, Debug)]
pub struct Subscription {
    pub ticker: Symbol,
    pub listing: Symbol,
}

impl Subscription {
    pub fn new(ticker: Symbol, listing: Symbol) -> Self {
        Self { ticker, listing }
    }
}

pub trait MarketDataConnector {
    /// Subscribe to set of symbols
    fn subscribe(&self, subscriptions: &[Subscription]) -> Result<()>;
}

pub mod test_util {

    use std::{
        collections::HashSet,
        sync::{
            atomic::{AtomicBool, Ordering},
            Arc,
        },
    };

    use eyre::{eyre, Result};
    use itertools::Itertools;
    use parking_lot::RwLock;

    use crate::{
        core::{
            bits::{Amount, PricePointEntry, Symbol},
            functional::{IntoObservableMany, MultiObserver, PublishMany},
        },
        market_data::market_data_connector::Subscription,
    };

    use super::{MarketDataConnector, MarketDataEvent};

    pub struct MockMarketDataConnector {
        observer: MultiObserver<Arc<MarketDataEvent>>,
        pub symbols: RwLock<HashSet<Symbol>>,
        pub is_connected: AtomicBool,
    }

    impl MockMarketDataConnector {
        pub fn new() -> Self {
            Self {
                observer: MultiObserver::new(),
                symbols: RwLock::new(HashSet::new()),
                is_connected: AtomicBool::new(false),
            }
        }

        /// receive market data from exchange (-> PriceTracker)
        pub fn notify_top_of_book(
            &self,
            symbol: Symbol,
            sequence_number: u64,
            best_bid_price: Amount,
            best_ask_price: Amount,
            best_bid_quantity: Amount,
            best_ask_quantity: Amount,
        ) {
            self.observer
                .publish_many(&Arc::new(MarketDataEvent::TopOfBook {
                    symbol,
                    sequence_number,
                    best_bid_price,
                    best_ask_price,
                    best_bid_quantity,
                    best_ask_quantity,
                }));
        }

        /// receive market data from exchange (-> PriceTracker)
        pub fn notify_trade(
            &self,
            symbol: Symbol,
            sequence_number: u64,
            price: Amount,
            quantity: Amount,
        ) {
            self.observer
                .publish_many(&Arc::new(MarketDataEvent::Trade {
                    symbol,
                    sequence_number,
                    price,
                    quantity,
                }));
        }

        /// receive market data from exchange (-> OrderBookManager)
        pub fn notify_full_order_book(
            &self,
            symbol: Symbol,
            sequence_number: u64,
            bid_updates: Vec<PricePointEntry>,
            ask_updates: Vec<PricePointEntry>,
        ) {
            self.observer
                .publish_many(&Arc::new(MarketDataEvent::OrderBookDelta {
                    symbol,
                    sequence_number,
                    bid_updates,
                    ask_updates,
                }));
        }

        /// Connect to exchange (-> Binance)
        pub fn connect(&mut self) {
            self.is_connected.store(true, Ordering::Relaxed);
        }

        pub fn subscribe_mock(&self, symbols: &[Symbol]) -> Result<()> {
            let subscriptions = symbols
                .iter()
                .map(|s| Subscription::new(s.clone(), "Mock".into()))
                .collect_vec();

            self.subscribe(&subscriptions)
        }
    }

    impl MarketDataConnector for MockMarketDataConnector {
        /// Subscribe to set of symbols (TBD: potentially async)
        fn subscribe(&self, susbscriptions: &[Subscription]) -> Result<()> {
            if self.is_connected.load(Ordering::Relaxed) {
                // this is mock, so we only want to know the symbols that user wanted to subscribe to
                let mut write_symbols = self.symbols.write();
                for Subscription { ticker, listing } in susbscriptions {
                    if listing.ne("Mock") {
                        Err(eyre!("Unknown listing: {}", listing))?;
                    }
                    write_symbols.insert(ticker.clone());
                }
                Ok(())
            } else {
                Err(eyre!("Cannot subscribe while not connected!"))
            }
        }
    }
    impl IntoObservableMany<Arc<MarketDataEvent>> for MockMarketDataConnector {
        fn get_multi_observer_mut(&mut self) -> &mut MultiObserver<Arc<MarketDataEvent>> {
            &mut self.observer
        }
    }
}

#[cfg(test)]
mod test {
    use std::{
        collections::HashSet,
        sync::{atomic::Ordering, Arc},
    };

    use rust_decimal::dec;

    use crate::{
        assert_decimal_approx_eq,
        core::{
            bits::{PricePointEntry, Symbol},
            functional::IntoObservableMany,
            test_util::{
                flag_mock_atomic_bool, get_mock_asset_name_1, get_mock_asset_name_2,
                get_mock_atomic_bool_pair, test_mock_atomic_bool,
            },
        },
        market_data::market_data_connector::{MarketDataConnector, MarketDataEvent},
    };

    use super::test_util::MockMarketDataConnector;

    /// Test MockMarketDataConnector
    ///
    /// These tests confirm that mock can be reliably used in other tests.
    ///
    #[test]
    fn test_mock_market_data_connector() {
        let mut connector = MockMarketDataConnector::new();

        assert!(matches!(
            connector.subscribe_mock(&[get_mock_asset_name_1(), get_mock_asset_name_2()]),
            Err(_)
        ));

        connector.connect();
        assert!(connector.is_connected.load(Ordering::Relaxed));

        assert!(matches!(
            connector.subscribe_mock(&[get_mock_asset_name_1(), get_mock_asset_name_2()]),
            Ok(())
        ));

        let expected = HashSet::from([get_mock_asset_name_1(), get_mock_asset_name_2()]);
        let got: HashSet<Symbol> = connector.symbols.read().iter().cloned().collect();

        assert_eq!(got, expected);

        let (called_for_tob, called_for_tob_inner) = get_mock_atomic_bool_pair();
        let (called_for_trade, called_for_trade_inner) = get_mock_atomic_bool_pair();
        let (called_for_fob, called_for_fob_inner) = get_mock_atomic_bool_pair();

        connector
            .get_multi_observer_mut()
            .add_observer_fn(move |e: &Arc<MarketDataEvent>| {
                match &**e {
                    MarketDataEvent::TopOfBook {
                        symbol,
                        sequence_number,
                        best_bid_price,
                        best_ask_price,
                        best_bid_quantity,
                        best_ask_quantity,
                    } => {
                        flag_mock_atomic_bool(&called_for_tob_inner);
                        assert_eq!(symbol, &get_mock_asset_name_1());
                        assert_eq!(best_bid_price, &dec!(1));
                        assert_eq!(best_ask_price, &dec!(2));
                        assert_eq!(best_bid_quantity, &dec!(10));
                        assert_eq!(best_ask_quantity, &dec!(20));
                    }
                    MarketDataEvent::Trade {
                        symbol,
                        sequence_number,
                        price,
                        quantity,
                    } => {
                        flag_mock_atomic_bool(&called_for_trade_inner);
                        assert_eq!(symbol, &get_mock_asset_name_2());
                        assert_eq!(price, &dec!(5));
                        assert_eq!(quantity, &dec!(2));
                    }
                    MarketDataEvent::OrderBookSnapshot {
                        symbol: _,
                        sequence_number,
                        bid_updates: _,
                        ask_updates: _,
                    } => {
                        todo!()
                    }
                    MarketDataEvent::OrderBookDelta {
                        symbol,
                        sequence_number,
                        bid_updates,
                        ask_updates,
                    } => {
                        flag_mock_atomic_bool(&called_for_fob_inner);
                        let tolerance = dec!(0.001);
                        assert_eq!(symbol, &get_mock_asset_name_1());
                        assert_decimal_approx_eq!(ask_updates[0].price, dec!(3.10), tolerance);
                        assert_decimal_approx_eq!(ask_updates[0].quantity, dec!(4.25), tolerance);
                        assert_decimal_approx_eq!(ask_updates[1].price, dec!(3.20), tolerance);
                        assert_decimal_approx_eq!(ask_updates[1].quantity, dec!(7.55), tolerance);
                        assert_eq!(bid_updates.len(), 0);
                    }
                };
            });

        connector.notify_top_of_book(
            get_mock_asset_name_1(),
            1,
            dec!(1),
            dec!(2),
            dec!(10),
            dec!(20),
        );

        assert!(test_mock_atomic_bool(&called_for_tob));

        connector.notify_trade(get_mock_asset_name_2(), 2, dec!(5), dec!(2));

        assert!(test_mock_atomic_bool(&called_for_trade));

        connector.notify_full_order_book(
            get_mock_asset_name_1(),
            3,
            vec![],
            vec![
                PricePointEntry {
                    price: dec!(3.10),
                    quantity: dec!(4.25),
                },
                PricePointEntry {
                    price: dec!(3.20),
                    quantity: dec!(7.55),
                },
            ],
        );

        assert!(test_mock_atomic_bool(&called_for_fob));
    }
}
