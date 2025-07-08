use eyre::{eyre, Result};
use std::{collections::HashMap, sync::Arc};

use symm_core::core::{
    bits::{Amount, Symbol},
    functional::{IntoObservableSingle, PublishSingle, SingleObserver},
};

use super::basket::{Basket, BasketDefinition};

pub enum BasketNotification {
    BasketAdded(Symbol, Arc<Basket>),
    BasketUpdated(Symbol, Arc<Basket>),
    BasketRemoved(Symbol),
}

/// Manages baskets, add, remove, update
pub struct BasketManager {
    baskets: HashMap<Symbol, Arc<Basket>>,
    observer: SingleObserver<BasketNotification>,
}

impl BasketManager {
    pub fn new() -> Self {
        Self {
            baskets: HashMap::new(),
            observer: SingleObserver::new(),
        }
    }

    pub fn get_basket(&self, symbol: &Symbol) -> Option<&Arc<Basket>> {
        self.baskets.get(symbol)
    }

    pub fn set_basket(&mut self, symbol: &Symbol, basket: &Arc<Basket>) {
        let mut event: Option<BasketNotification> = None;

        self.baskets
            .entry(symbol.clone())
            .and_modify(|value| {
                if self.observer.has_observer() {
                    event = Some(BasketNotification::BasketUpdated(
                        symbol.clone(),
                        basket.clone(),
                    ));
                }
                *value = basket.clone();
            })
            .or_insert_with(|| {
                if self.observer.has_observer() {
                    event = Some(BasketNotification::BasketAdded(
                        symbol.clone(),
                        basket.clone(),
                    ));
                }
                basket.clone()
            });

        if let Some(e) = event {
            self.observer.publish_single(e);
        }
    }

    pub fn set_basket_from_definition(
        &mut self,
        symbol: Symbol,
        basket_definition: BasketDefinition,
        individual_prices: &HashMap<Symbol, Amount>,
        target_price: Amount,
    ) -> Result<()> {
        let basket = Arc::new(Basket::new_with_prices(
            basket_definition,
            individual_prices,
            target_price,
        )?);
        self.set_basket(&symbol, &basket);
        Ok(())
    }

    pub fn remove_basket(&mut self, symbol: &Symbol) -> Result<()> {
        self.baskets
            .remove(&symbol)
            .ok_or(eyre!("Basket does not exist {}", symbol))?;

        self.observer
            .publish_single(BasketNotification::BasketRemoved(symbol.clone()));
        Ok(())
    }
}

impl IntoObservableSingle<BasketNotification> for BasketManager {
    fn get_single_observer_mut(&mut self) -> &mut SingleObserver<BasketNotification> {
        &mut self.observer
    }
}

#[cfg(test)]
mod tests {
    use crate::index::basket::AssetWeight;
    use crate::index::basket::BasketDefinition;
    use crate::index::basket_manager::{BasketManager, BasketNotification};
    use eyre::{eyre, Result};
    use parking_lot::RwLock;
    use std::collections::VecDeque;
    use std::{collections::HashMap, sync::Arc};
    use symm_core::{
        assert_deque_single_matches, assert_deque_single_matches_and_return,
        assert_hashmap_amounts_eq,
        assets::asset::Asset,
        core::{
            bits::{Amount, Symbol},
            functional::IntoObservableSingle,
        },
    };

    #[test]
    fn test_basket_manager() -> Result<()> {
        let assert_tolerance = "0.00001".try_into()?;

        let symbol_index: Symbol = "IDX1".into();

        // Define some assets - they will stay
        let asset_btc = Arc::new(Asset::new("BTC".into(), "BINANCE".into()));
        let asset_eth = Arc::new(Asset::new("ETH".into(), "BINANCE".into()));
        let asset_sol = Arc::new(Asset::new("SOL".into(), "BINANCE".into()));

        // Define basket - it will be consumed when we create Basket
        let basket_definition = BasketDefinition::try_new([
            AssetWeight::new(asset_btc.clone(), "0.25".try_into()?),
            AssetWeight::new(asset_eth.clone(), "0.75".try_into()?),
        ])?;

        tracing::info!("basket_definition = {}", basket_definition);

        // Tell reference prices for assets for in basket quantities computation
        let individual_prices: HashMap<Symbol, Amount> = [
            (asset_btc.ticker.clone(), "50000.0".try_into()?),
            (asset_eth.ticker.clone(), "6000.0".try_into()?),
        ]
        .into();

        // Set target price for computing actual quantites for the basket
        let target_price = "10000.0".try_into()?;

        // We're testing that BasketManager will create a basket
        let mut basket_manager = BasketManager::new();

        let notifications = Arc::new(RwLock::new(VecDeque::new()));
        let notifiations_2 = notifications.clone();

        basket_manager
            .get_single_observer_mut()
            .set_observer_fn(move |notification| notifiations_2.write().push_back(notification));

        //
        // I. Test that we can create new basket with weights, prices, and target price
        //
        // We should be receiving BasketNotification::BasketAdded(symbol, new_basket), where
        // quantities in the new_basket should have been calculated to match the target price
        // and using hte weights provided. When we receive notification, the basket manager
        // should be already in new state.
        //

        // Create actual basket consuming the definition
        basket_manager.set_basket_from_definition(
            symbol_index.clone(),
            basket_definition,
            &individual_prices,
            target_price,
        )?;

        // Check that we received notification of basket being added, and verify the assigned quantites
        if let Some(BasketNotification::BasketAdded(_, basket)) = assert_deque_single_matches_and_return!(notifications.write(), BasketNotification::BasketAdded(symbol, _)
            if *symbol == symbol_index)
        {
            let expected: HashMap<Symbol, Amount> = [
                ("BTC".into(), "0.05".try_into()?),
                ("ETH".into(), "1.25".try_into()?),
            ]
            .into();

            let quantites: HashMap<Symbol, Amount> = basket
                .basket_assets
                .iter()
                .map(|ba| (ba.weight.asset.ticker.clone(), ba.quantity))
                .collect();

            assert_hashmap_amounts_eq!(quantites, expected, assert_tolerance);
        }

        // Tell reference prices for assets for in basket quantities computation
        let individual_prices_updated: HashMap<Symbol, Amount> = [
            (asset_btc.ticker.clone(), "55000.0".try_into()?),
            (asset_eth.ticker.clone(), "6250.0".try_into()?),
            (asset_sol.ticker.clone(), "250.0".try_into()?),
        ]
        .into();

        // Define basket - it will be consumed when we create Basket
        let basket_definition_updated = BasketDefinition::try_new([
            AssetWeight::new(asset_btc.clone(), "0.25".try_into()?),
            AssetWeight::new(asset_eth.clone(), "0.75".try_into()?),
            AssetWeight::new(asset_sol, "0.15".try_into()?),
        ])?;

        // Get current price of all the assets in the basket
        let target_price_updated = basket_manager
            .get_basket(&symbol_index)
            .ok_or(eyre!("Basket not found"))?
            .get_current_price(&individual_prices_updated)?;

        //
        // II. Test that we can update(*) basket with weights, prices, and target price
        //
        // (*) We won't update the existing basket, and instead we will replace it with new one.
        // We should be receiving BasketNotification::BasketUpdated(symbol, new_basket), where
        // quantities in the new_basket should have been calculated to match the target price
        // and using hte weights provided. When we receive notification, the basket manager
        // should be already in new state.
        //

        // Create actual basket consuming the definition
        basket_manager.set_basket_from_definition(
            symbol_index.clone(),
            basket_definition_updated,
            &individual_prices_updated,
            target_price_updated,
        )?;

        // Check that we received notification of basket being updated, and verify the assigned quantites
        if let Some(BasketNotification::BasketUpdated(_, basket)) = assert_deque_single_matches_and_return!(notifications.write(), BasketNotification::BasketUpdated(symbol, _)
            if *symbol == symbol_index)
        {
            let expected: HashMap<Symbol, Amount> = [
                ("BTC".into(), "0.0417490118577075098814229249".try_into()?),
                ("ETH".into(), "1.1021739130434782608695652174".try_into()?),
                ("SOL".into(), "5.5108695652173913043478260879".try_into()?),
            ]
            .into();

            let quantites: HashMap<Symbol, Amount> = basket
                .basket_assets
                .iter()
                .map(|ba| (ba.weight.asset.ticker.clone(), ba.quantity))
                .collect();

            assert_hashmap_amounts_eq!(quantites, expected, assert_tolerance);
        }

        //
        // III. Test that we can remove basket
        //
        // We should be receiving BasketNotification::BasketRemoved(symbol).
        // When we receive notification, the basket manager should be already in new state.
        //

        basket_manager.remove_basket(&symbol_index)?;

        assert_deque_single_matches!(notifications.write(), BasketNotification::BasketRemoved(symbol)
            if symbol == symbol_index);

        Ok(())
    }
}
