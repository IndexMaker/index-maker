use std::sync::Arc;

use crate::index::basket::{Basket, BasketDefinition};

/// call blockchain methods, receive blockchain events

pub enum ChainNotification {
    CuratorWeightsSet(BasketDefinition)
}


pub trait ChainConnector {
    fn set_observer(&mut self, observer: Box<dyn Fn(ChainNotification)>);
    fn solver_weights_set(&self, basket: Arc<Basket>);
}

pub struct MockChainConnector {
    pub observer: Option<Box<dyn Fn(ChainNotification)>>,
    pub internal_observer: Box<dyn Fn(Arc<Basket>)>
}

impl MockChainConnector {
    pub fn new(internal_observer: Box<dyn Fn(Arc<Basket>)>) -> Self {
        Self { observer:  None, internal_observer }
    }

    pub fn publish(&mut self, notification: ChainNotification) {
        if let Some(observer) = &mut self.observer {
            (*observer)(notification)
        }
    }
}

impl ChainConnector for MockChainConnector {
    fn set_observer(&mut self, observer: Box<dyn Fn(ChainNotification)>) {
        self.observer = Some(observer);
    }

    fn solver_weights_set(&self, basket: Arc<Basket>) {
        (*self.internal_observer)(basket);
    }
}

#[cfg(test)]
mod tests {
    use std::{collections::HashMap, sync::{mpsc::channel, Arc}};

    use parking_lot::RwLock;

    use crate::{
        assert_hashmap_amounts_eq, assets::asset::Asset,
        blockchain::chain_connector::ChainNotification, core::bits::{Amount, Symbol}, 
        index::{basket::{AssetWeight, BasketDefinition}, basket_manager::{BasketManager, BasketNotification}}
    };

    use super::{ChainConnector, MockChainConnector};

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
        let (tx, rx) = channel::<Box<dyn FnOnce()>>();
        let tx_2 = tx.clone();
        let (tx_end, rx_end) = channel::<bool>();

        let basket_manager = Arc::new(RwLock::new(BasketManager::new()));

        let assert_tolerance: Amount = "0.00001".try_into().unwrap();
        let assert_tolerance_2 = assert_tolerance.clone();

        // Define some assets - they will stay
        let asset_btc = Arc::new(Asset::new("BTC".into()));
        let asset_eth = Arc::new(Asset::new("ETH".into()));

        // Define basket - it will be consumed when we create Basket
        let basket_definition = BasketDefinition::try_new([
            AssetWeight::new(asset_btc.clone(), "0.25".try_into().unwrap()),
            AssetWeight::new(asset_eth.clone(), "0.75".try_into().unwrap()) 
        ]).unwrap();

        let mock_chain_connection = Arc::new(RwLock::new(MockChainConnector::new(Box::new(move |basket| {
            let tx_end = tx_end.clone();
            tx.send(Box::new(move || {
                let expected: HashMap<Symbol, Amount> = [
                    ("BTC".into(), "0.05".try_into().unwrap()),
                    ("ETH".into(), "1.25".try_into().unwrap()) ].into();

                let quantites: HashMap<Symbol, Amount> = basket.basket_assets.iter()
                    .map(|ba| (ba.weight.asset.name.clone(), ba.quantity))
                    .collect();

                assert_hashmap_amounts_eq!(quantites, expected, assert_tolerance_2);

                tx_end.send(true).unwrap();
            })).unwrap();
        }))));

        let mock_chain_connection_2 = mock_chain_connection.clone();

        basket_manager.write().set_basket_observer(Box::new(move |notification| {
            match notification {
                BasketNotification::BasketAdded(_, basket) => {
                    mock_chain_connection_2.write().solver_weights_set(basket);
                }
                _ => panic!("Expected basket add notification")
            };
        }));

        mock_chain_connection.write().set_observer(Box::new(move |notification| {
            assert!(matches!(notification, ChainNotification::CuratorWeightsSet(_)));
            let basket_manager = basket_manager.clone();
            match notification {
                ChainNotification::CuratorWeightsSet(basket_definition) => {
                    tx_2.send(Box::new(move || {
                        let expected: HashMap<Symbol, Amount> = [
                                        ("BTC".into(), "0.25".try_into().unwrap()),
                                        ("ETH".into(), "0.75".try_into().unwrap()) ].into();

                        let weights: HashMap<Symbol, Amount> = basket_definition.weights.iter()
                                        .map(|w| (w.asset.name.clone(), w.weight)).collect();

                        assert_hashmap_amounts_eq!(weights, expected, assert_tolerance);

                        // Tell reference prices for assets for in basket quantities computation
                        let individual_prices: HashMap<Symbol, Amount> = [
                            ("BTC".into(), "50000.0".try_into().unwrap()),
                            ("ETH".into(), "6000.0".try_into().unwrap())
                        ].into();

                        // Set target price for computing actual quantites for the basket
                        let target_price = "10000.0".try_into().unwrap();

                        basket_manager.write().set_basket_from_definition("SYB".into(), 
                            basket_definition, &individual_prices, target_price).unwrap();

                    })).unwrap();
                }
            }
        }));

        mock_chain_connection.write().publish(super::ChainNotification::CuratorWeightsSet(basket_definition));

        rx.try_iter().for_each(|f| f());

        assert_eq!(rx_end.recv(), Ok(true));
    }
}

