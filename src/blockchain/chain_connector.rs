use std::sync::Arc;

use crate::{
    core::bits::{Address, Amount, Symbol},
    index::basket::{Basket, BasketDefinition},
};

/// call blockchain methods, receive blockchain events

/// On-chain event
pub enum ChainNotification {
    CuratorWeightsSet(BasketDefinition), // ...more
}

/// Connects to some Blockchain
pub trait ChainConnector {
    fn solver_weights_set(&self, basket: Arc<Basket>);
    fn mint_index(&self, symbol: Symbol, quantity: Amount, receipient: Address);
    // ...more
}

/// Mock implementations of the traits
#[cfg(test)]
pub mod test_util {
    use std::sync::Arc;

    use crate::{
        core::{
            bits::{Address, Amount, Symbol},
            functional::SingleObserver,
        },
        index::basket::{Basket, BasketDefinition},
    };

    use super::{ChainConnector, ChainNotification};

    /// Internal event so that we can write tests confirming that internal logic is reached
    pub enum MockChainInternalNotification {
        SolverWeightsSet(Arc<Basket>),
        MintIndex {
            symbol: Symbol,
            quantity: Amount,
            receipient: Address,
        },
    }

    pub struct MockChainConnector {
        pub observer: SingleObserver<ChainNotification>,
        pub internal_observer: SingleObserver<MockChainInternalNotification>,
    }

    impl MockChainConnector {
        pub fn new() -> Self {
            Self {
                observer: SingleObserver::new(),
                internal_observer: SingleObserver::new(),
            }
        }

        /// Receive events from chain
        pub fn connect(&mut self) {
            // connect to blockchain
        }

        /// Notify about on chain events
        pub fn notify_curator_weights_set(&self, basket_definition: BasketDefinition) {
            self.observer
                .publish_single(super::ChainNotification::CuratorWeightsSet(
                    basket_definition,
                ));
        }

        pub fn new_with_observers(
            observer: SingleObserver<ChainNotification>,
            internal_observer: SingleObserver<MockChainInternalNotification>,
        ) -> Self {
            Self {
                observer,
                internal_observer,
            }
        }
    }

    impl ChainConnector for MockChainConnector {
        fn solver_weights_set(&self, basket: Arc<Basket>) {
            self.internal_observer
                .publish_single(MockChainInternalNotification::SolverWeightsSet(basket));
        }

        fn mint_index(&self, symbol: Symbol, quantity: Amount, receipient: Address) {
            self.internal_observer
                .publish_single(MockChainInternalNotification::MintIndex {
                    symbol,
                    quantity,
                    receipient,
                });
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{collections::HashMap, sync::Arc};

    use parking_lot::RwLock;

    use crate::{
        assert_hashmap_amounts_eq,
        blockchain::chain_connector::{ChainConnector, ChainNotification},
        core::{
            bits::{Amount, Symbol},
            functional::SingleObserver,
            test_util::*,
        },
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
        let (tx, rx) = get_mock_channel::<Box<dyn FnOnce()>>();
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
            AssetWeight::new(asset_btc.clone(), get_mock_decimal("0.25")),
            AssetWeight::new(asset_eth.clone(), get_mock_decimal("0.75")),
        ])
        .unwrap();

        let chain_observer = SingleObserver::new_with_observer(Box::new(move |notification| {
            assert!(matches!(
                notification,
                ChainNotification::CuratorWeightsSet(_)
            ));
            let basket_manager = basket_manager.clone();
            match notification {
                ChainNotification::CuratorWeightsSet(basket_definition) => {
                    tx_2.send(Box::new(move || {
                        let expected: HashMap<Symbol, Amount> = [
                            (get_mock_asset_name_1(), get_mock_decimal("0.25")),
                            (get_mock_asset_name_2(), get_mock_decimal("0.75")),
                        ]
                        .into();

                        let weights: HashMap<Symbol, Amount> = basket_definition
                            .weights
                            .iter()
                            .map(|w| (w.asset.name.clone(), w.weight))
                            .collect();

                        assert_hashmap_amounts_eq!(weights, expected, assert_tolerance_2);

                        // Tell reference prices for assets for in basket quantities computation
                        let individual_prices: HashMap<Symbol, Amount> = [
                            (get_mock_asset_name_1(), get_mock_decimal("50000.0")),
                            (get_mock_asset_name_2(), get_mock_decimal("6000.0")),
                        ]
                        .into();

                        // Set target price for computing actual quantites for the basket
                        let target_price = get_mock_decimal("10000.0");

                        basket_manager
                            .write()
                            .set_basket_from_definition(
                                "SYB".into(),
                                basket_definition,
                                &individual_prices,
                                target_price,
                            )
                            .unwrap();
                    }))
                    .unwrap();
                }
            }
        }));

        let internal_observer = SingleObserver::new_with_observer(Box::new(move |notification| {
            if let MockChainInternalNotification::SolverWeightsSet(basket) = notification {
                let tx_end = tx_end.clone();
                tx.send(Box::new(move || {
                    let expected: HashMap<Symbol, Amount> = [
                        (get_mock_asset_name_1(), get_mock_decimal("0.05")),
                        (get_mock_asset_name_2(), get_mock_decimal("1.25")),
                    ]
                    .into();

                    let quantites: HashMap<Symbol, Amount> = basket
                        .basket_assets
                        .iter()
                        .map(|ba| (ba.weight.asset.name.clone(), ba.quantity))
                        .collect();

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
            .set_basket_observer(Box::new(move |notification| {
                match notification {
                    BasketNotification::BasketAdded(_, basket) => {
                        mock_chain_connection_2.write().solver_weights_set(basket);
                    }
                    _ => panic!("Expected basket add notification"),
                };
            }));

        mock_chain_connection.write().mint_index(
            get_mock_index_name_1(),
            get_mock_decimal("0.5"),
            get_mock_address_1(),
        );

        mock_chain_connection.write().connect();
        mock_chain_connection
            .read()
            .notify_curator_weights_set(basket_definition);

        rx.try_iter().for_each(|f| f());

        assert_eq!(rx_end.recv(), Ok(true));
    }
}
