use std::sync::Arc;

use chrono::{DateTime, Utc};

use derive_with_baggage::WithBaggage;
use opentelemetry::propagation::Injector;
use symm_core::core::telemetry::{TracingData, WithBaggage};

use symm_core::core::{
    bits::{Address, Amount, Symbol},
    functional::IntoObservableSingleVTable,
};

use crate::index::basket::{Basket, BasketDefinition};

/// call blockchain methods, receive blockchain events

/// On-chain event
#[derive(WithBaggage)]
pub enum ChainNotification {
    CuratorWeightsSet(Symbol, BasketDefinition), // ...more
    Deposit {
        #[baggage]
        chain_id: u32,

        #[baggage]
        address: Address,

        amount: Amount,
        timestamp: DateTime<Utc>,
    },
    WithdrawalRequest {
        #[baggage]
        chain_id: u32,

        #[baggage]
        address: Address,

        amount: Amount,
        timestamp: DateTime<Utc>,
    },
    ChainConnected {
        chain_id: u32,
        timestamp: DateTime<Utc>,
    },
    ChainDisconnected {
        chain_id: u32,
        reason: String,
        timestamp: DateTime<Utc>,
    },
}

/// Connects to some Blockchain
pub trait ChainConnector: IntoObservableSingleVTable<ChainNotification> {
    fn solver_weights_set(&self, symbol: Symbol, basket: Arc<Basket>);
    fn mint_index(
        &self,
        chain_id: u32,
        symbol: Symbol,
        quantity: Amount,
        receipient: Address,
        execution_price: Amount,
        execution_time: DateTime<Utc>,
    );
    fn burn_index(&self, chain_id: u32, symbol: Symbol, quantity: Amount, receipient: Address);
    fn withdraw(
        &self,
        chain_id: u32,
        receipient: Address,
        amount: Amount,
        execution_price: Amount,
        execution_time: DateTime<Utc>,
    );
}

/// Mock implementations of the traits
pub mod test_util {
    use std::sync::Arc;

    use chrono::{DateTime, Utc};

    use symm_core::core::{
        bits::{Address, Amount, Symbol},
        functional::{
            IntoObservableSingle, IntoObservableSingleVTable, NotificationHandlerOnce,
            PublishSingle, SingleObserver,
        },
    };

    use crate::index::basket::{Basket, BasketDefinition};

    use super::{ChainConnector, ChainNotification};

    /// Internal event so that we can write tests confirming that internal logic is reached
    pub enum MockChainInternalNotification {
        SolverWeightsSet(Symbol, Arc<Basket>),
        MintIndex {
            chain_id: u32,
            symbol: Symbol,
            quantity: Amount,
            receipient: Address,
            execution_price: Amount,
            execution_time: DateTime<Utc>,
        },
        BurnIndex {
            chain_id: u32,
            symbol: Symbol,
            quantity: Amount,
            receipient: Address,
        },
        Withdraw {
            chain_id: u32,
            receipient: Address,
            amount: Amount,
            execution_price: Amount,
            execution_time: DateTime<Utc>,
        },
    }

    pub struct MockChainConnector {
        observer: SingleObserver<ChainNotification>,
        pub implementor: SingleObserver<MockChainInternalNotification>,
    }

    impl MockChainConnector {
        pub fn new() -> Self {
            Self {
                observer: SingleObserver::new(),
                implementor: SingleObserver::new(),
            }
        }

        /// Receive events from chain
        pub fn connect(&mut self) {
            // connect to blockchain
        }

        /// Notify about on chain events
        pub fn notify_curator_weights_set(
            &self,
            symbol: Symbol,
            basket_definition: BasketDefinition,
        ) {
            self.observer
                .publish_single(super::ChainNotification::CuratorWeightsSet(
                    symbol,
                    basket_definition,
                ));
        }

        pub fn notify_deposit(
            &self,
            chain_id: u32,
            address: Address,
            amount: Amount,
            timestamp: DateTime<Utc>,
        ) {
            self.observer.publish_single(ChainNotification::Deposit {
                chain_id,
                address,
                amount,
                timestamp,
            });
        }

        pub fn notify_withdrawal_request(
            &self,
            chain_id: u32,
            address: Address,
            amount: Amount,
            timestamp: DateTime<Utc>,
        ) {
            self.observer
                .publish_single(ChainNotification::WithdrawalRequest {
                    chain_id,
                    address,
                    amount,
                    timestamp,
                });
        }

        pub fn new_with_observers(
            observer: SingleObserver<ChainNotification>,
            implementor: SingleObserver<MockChainInternalNotification>,
        ) -> Self {
            Self {
                observer,
                implementor,
            }
        }
    }

    impl IntoObservableSingle<ChainNotification> for MockChainConnector {
        fn get_single_observer_mut(&mut self) -> &mut SingleObserver<ChainNotification> {
            &mut self.observer
        }
    }

    impl IntoObservableSingleVTable<ChainNotification> for MockChainConnector {
        fn set_observer(&mut self, observer: Box<dyn NotificationHandlerOnce<ChainNotification>>) {
            self.get_single_observer_mut().set_observer(observer);
        }
    }

    impl ChainConnector for MockChainConnector {
        fn solver_weights_set(&self, symbol: Symbol, basket: Arc<Basket>) {
            self.implementor
                .publish_single(MockChainInternalNotification::SolverWeightsSet(
                    symbol, basket,
                ));
        }

        fn mint_index(
            &self,
            chain_id: u32,
            symbol: Symbol,
            quantity: Amount,
            receipient: Address,
            execution_price: Amount,
            execution_time: DateTime<Utc>,
        ) {
            self.implementor
                .publish_single(MockChainInternalNotification::MintIndex {
                    chain_id,
                    symbol,
                    quantity,
                    receipient,
                    execution_price,
                    execution_time,
                });
        }

        fn burn_index(&self, chain_id: u32, symbol: Symbol, quantity: Amount, receipient: Address) {
            self.implementor
                .publish_single(MockChainInternalNotification::BurnIndex {
                    chain_id,
                    symbol,
                    quantity,
                    receipient,
                });
        }

        fn withdraw(
            &self,
            chain_id: u32,
            receipient: Address,
            amount: Amount,
            execution_price: Amount,
            execution_time: DateTime<Utc>,
        ) {
            self.implementor
                .publish_single(MockChainInternalNotification::Withdraw {
                    chain_id,
                    receipient,
                    amount,
                    execution_price,
                    execution_time,
                });
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{collections::HashMap, sync::Arc};

    use chrono::Utc;
    use parking_lot::RwLock;
    use rust_decimal::dec;

    use symm_core::{
        assert_hashmap_amounts_eq,
        core::{
            bits::{Amount, Symbol},
            functional::{IntoObservableSingle, SingleObserver},
            test_util::*,
        },
    };

    use crate::{
        blockchain::chain_connector::{ChainConnector, ChainNotification},
        index::{
            basket::{AssetWeight, BasketDefinition},
            basket_manager::{BasketManager, BasketNotification},
        },
    };

    use super::test_util::*;

    // This test demonstrates that it is possible to:
    //  - Receive Curator weights as BasketDefinition
    //  - Emit an event containing BasketDefinition
    //  - Consume that BasketDefinition to create Basket
    //  - Observe Basket Added event
    // The flow cannot be achieved synchronously, as that would result in dead-lock.
    // We defer event handling outside of write lock. Note that defered action must be FnOnce.
    //
    #[test]
    fn test_mock_chain_connection() {
        let (tx, rx) = get_mock_defer_channel();
        let tx_2 = tx.clone();
        let (tx_end, rx_end) = get_mock_channel::<bool>();

        let basket_manager = Arc::new(RwLock::new(BasketManager::new()));
        let basket_manager_3 = basket_manager.clone();

        let assert_tolerance_1 = get_mock_tolerance();
        let assert_tolerance_2 = assert_tolerance_1.clone();

        // Define some assets - they will stay
        let asset_btc = get_mock_asset_1_arc();
        let asset_eth = get_mock_asset_2_arc();

        // Define basket - it will be consumed when we create Basket
        let basket_definition = BasketDefinition::try_new([
            AssetWeight::new(asset_btc.clone(), dec!(0.25)),
            AssetWeight::new(asset_eth.clone(), dec!(0.75)),
        ])
        .unwrap();

        let chain_observer = SingleObserver::new_with_observer(Box::new(move |notification| {
            assert!(matches!(
                notification,
                ChainNotification::CuratorWeightsSet(_, _)
            ));
            let basket_manager = basket_manager.clone();
            match notification {
                ChainNotification::CuratorWeightsSet(symbol, basket_definition) => {
                    tx_2.send(Box::new(move || {
                        let expected: HashMap<Symbol, Amount> = [
                            (get_mock_asset_name_1(), dec!(0.25)),
                            (get_mock_asset_name_2(), dec!(0.75)),
                        ]
                        .into();

                        let weights: HashMap<Symbol, Amount> = basket_definition
                            .weights
                            .iter()
                            .map(|w| (w.asset.ticker.clone(), w.weight))
                            .collect();

                        assert_eq!(symbol, get_mock_index_name_1());
                        assert_hashmap_amounts_eq!(weights, expected, assert_tolerance_2);

                        // Tell reference prices for assets for in basket quantities computation
                        let individual_prices: HashMap<Symbol, Amount> = [
                            (get_mock_asset_name_1(), dec!(50000.0)),
                            (get_mock_asset_name_2(), dec!(6000.0)),
                        ]
                        .into();

                        // Set target price for computing actual quantites for the basket
                        let target_price = dec!(10000.0);

                        basket_manager
                            .write()
                            .set_basket_from_definition(
                                get_mock_index_name_1(),
                                basket_definition,
                                &individual_prices,
                                target_price,
                            )
                            .unwrap();
                    }))
                    .unwrap();
                }
                _ => (),
            }
        }));

        let internal_observer = SingleObserver::new_with_observer(Box::new(move |notification| {
            if let MockChainInternalNotification::SolverWeightsSet(symbol, basket) = notification {
                let tx_end = tx_end.clone();
                tx.send(Box::new(move || {
                    let expected: HashMap<Symbol, Amount> = [
                        (get_mock_asset_name_1(), dec!(0.05)),
                        (get_mock_asset_name_2(), dec!(1.25)),
                    ]
                    .into();

                    let quantites: HashMap<Symbol, Amount> = basket
                        .basket_assets
                        .iter()
                        .map(|ba| (ba.weight.asset.ticker.clone(), ba.quantity))
                        .collect();

                    assert_eq!(symbol, get_mock_index_name_1());
                    assert_hashmap_amounts_eq!(quantites, expected, assert_tolerance_1);

                    tx_end.send(true).unwrap();
                }))
                .unwrap();
            }
        }));

        let mock_chain_connection = Arc::new(RwLock::new(MockChainConnector::new_with_observers(
            chain_observer,
            internal_observer,
        )));

        let mock_chain_connection_2 = mock_chain_connection.clone();

        basket_manager_3
            .write()
            .get_single_observer_mut()
            .set_observer_fn(Box::new(move |notification| {
                match notification {
                    BasketNotification::BasketAdded(symbol, basket) => {
                        mock_chain_connection_2
                            .write()
                            .solver_weights_set(symbol, basket);
                    }
                    _ => panic!("Expected basket add notification"),
                };
            }));

        mock_chain_connection.write().mint_index(
            1,
            get_mock_index_name_1(),
            dec!(0.5),
            get_mock_address_1(),
            dec!(0.5),
            Utc::now(),
        );

        mock_chain_connection.write().connect();
        mock_chain_connection
            .read()
            .notify_curator_weights_set(get_mock_index_name_1(), basket_definition);

        run_mock_deferred(&rx);

        assert_eq!(rx_end.recv(), Ok(true));
    }
}
