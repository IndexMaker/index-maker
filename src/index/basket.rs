use eyre::{eyre, Report, Result};
use itertools::Itertools;
use rust_decimal::Decimal;
use safe_math::safe;
use std::{collections::HashMap, fmt::Display, sync::Arc};

use symm_core::{
    assets::asset::Asset,
    core::{
        bits::{Amount, Symbol},
        decimal_ext::DecimalExt,
    },
};

/// An asset with its associated weight.
///
/// This struct is used for BasketDefinition and Basket.
#[derive(Clone)]
pub struct AssetWeight {
    pub asset: Arc<Asset>,
    pub weight: Amount,
}

impl AssetWeight {
    pub fn new(asset: Arc<Asset>, weight: Amount) -> Self {
        Self { asset, weight }
    }
}

/// The definition of the Basket.
///
/// This struct is used to define weights for each asset to be then quantified in the Basket.
///
/// The struct is intended to be used for Read-Only purpose, and to make an update this
/// struct needs to be burned, and new struct needs to be created.
#[derive(Clone)]
pub struct BasketDefinition {
    pub weights: Vec<AssetWeight>,
}

impl BasketDefinition {
    /// This method normalizes provided weights, and errors if numeric overflow happened
    pub fn try_new<I>(weights: I) -> Result<Self>
    where
        I: IntoIterator<Item = AssetWeight>,
    {
        let weights = weights.into_iter().collect_vec();
        let total_weight = weights
            .iter()
            .try_fold(Amount::ZERO, |a, x| safe!(a + x.weight))
            .ok_or(eyre!("Numeric overflow"))?;
        let weights = weights
            .into_iter()
            .map(|w| {
                if let Some(weight) = safe!(w.weight / total_weight) {
                    Some(AssetWeight::new(w.asset, weight))
                } else {
                    None
                }
            })
            .collect_vec();
        if weights.iter().all(Option::is_some) {
            Ok(Self {
                weights: weights.into_iter().map(Option::unwrap).collect_vec(),
            })
        } else {
            Err(eyre!("Numeric overflow while division"))
        }
    }
}

/// Consume BaketDefinition and get AssetWeights from it.
impl From<BasketDefinition> for Vec<AssetWeight> {
    fn from(basket_definition: BasketDefinition) -> Self {
        basket_definition.weights
    }
}

/// Developer friendly representation of BasketDefinition.
///
/// Should not be used for anything else than logging.
impl Display for BasketDefinition {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "BasketDefinition[{}]",
            self.weights
                .iter()
                .map(|w| format!("{}: {:.7}", w.asset.name, w.weight))
                .join(", ")
        )
    }
}

/// An asset within the Basket.
///
/// This struct associates quantity of an asset for containing basket.
/// The quantity is calculated based on price, which is also memorised here
/// together with the original weight assigned to asset.
#[derive(Clone)]
pub struct BasketAsset {
    pub weight: AssetWeight,
    pub price: Amount,
    pub quantity: Amount,
}

/// A Basket of assets.
///
/// Each asset has quantity assigned based on target_price of the basket, asset
/// price and weight.
///
/// The struct is intended to be used for Read-Only purpose, and to make an update this
/// struct needs to be burned, and new struct needs to be created.
pub struct Basket {
    pub basket_assets: Vec<BasketAsset>,
    pub target_price: Amount,
}

impl Basket {
    /// Create new basket consuming BasketDefinition, and using individual_prices of assets
    /// and target_price of the basket, calculate quantites and assign to assets.
    pub fn new_with_prices(
        basket_definition: BasketDefinition,
        individual_prices: &HashMap<Symbol, Amount>,
        target_price: Amount,
    ) -> Result<Basket> {
        // Map definition to locked quantities
        let basket_assets: Vec<BasketAsset> = basket_definition
            .weights
            .into_iter()
            .map(|weight| {
                let price: &Amount = individual_prices
                    .get(&weight.asset.name)
                    .unwrap_or(&Amount::ZERO);
                let quantity =
                    safe!(safe!(target_price / *price) * weight.weight).unwrap_or_default();
                BasketAsset {
                    weight,
                    price: *price,
                    quantity,
                }
            })
            .collect();

        // Complain about assets with prices
        let unpriced_assets: Vec<&BasketAsset> = basket_assets
            .iter()
            .filter(|asset| asset.price.is_zero())
            .collect();
        if !unpriced_assets.is_empty() {
            return Err(eyre!(format!(
                "Unknown price of the assets: {}",
                unpriced_assets
                    .into_iter()
                    .map(|a| a.weight.asset.name.as_ref())
                    .join(", ")
            )));
        }

        // Complain about numeric overflow during quantity calculation
        let overflown_assets: Vec<&BasketAsset> = basket_assets
            .iter()
            .filter(|asset| asset.quantity.is_zero())
            .collect();
        if !overflown_assets.is_empty() {
            return Err(eyre!(format!(
                "Numeric overflow while computing quantity for assets: {}",
                overflown_assets
                    .into_iter()
                    .map(|a| a.weight.asset.name.as_ref())
                    .join(", ")
            )));
        }

        Ok(Self {
            basket_assets,
            target_price,
        })
    }

    pub fn get_current_price(&self, individual_prices: &HashMap<Symbol, Amount>) -> Result<Amount> {
        let prices = self
            .basket_assets
            .iter()
            .map(|ba| {
                (
                    &ba.weight.asset.name,
                    individual_prices
                        .get(&ba.weight.asset.name)
                        .and_then(|x| safe!(*x * ba.quantity)),
                )
            })
            .collect_vec();

        let unpriced_assets = prices.iter().filter(|p| p.1.is_none()).collect_vec();
        if !unpriced_assets.is_empty() {
            return Err(eyre!(format!(
                "Unknown price of the assets: {}",
                unpriced_assets.into_iter().map(|a| a.0).join(", ")
            )));
        }

        prices
            .iter()
            .map(|x| x.1.unwrap())
            .try_fold(Decimal::ZERO, |a, x| safe!(x + a))
            .ok_or(eyre!(
                "Numeric overflow while computing current price of the basket"
            ))
    }
}

impl Display for Basket {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Basket[{}; {}]",
            self.target_price,
            self.basket_assets
                .iter()
                .map(|ba| format!(
                    "{:.7}{} @ ${:.7} (w={:.7})",
                    ba.quantity, ba.weight.asset.name, ba.price, ba.weight.weight
                ))
                .join(", ")
        )
    }
}

impl TryFrom<Basket> for BasketDefinition {
    type Error = Report;
    fn try_from(basket: Basket) -> Result<Self> {
        BasketDefinition::try_new(basket.basket_assets.into_iter().map(|ba| ba.weight))
    }
}

impl From<Basket> for Vec<AssetWeight> {
    fn from(basket: Basket) -> Self {
        basket
            .basket_assets
            .into_iter()
            .map(|ba| ba.weight)
            .collect_vec()
    }
}

#[cfg(test)]
mod tests {
    use crate::index::basket::{AssetWeight, Basket, BasketDefinition};
    use eyre::Result;
    use rust_decimal::dec;
    use std::{collections::HashMap, sync::Arc};
    use symm_core::{
        assert_decimal_approx_eq,
        assets::asset::Asset,
        core::bits::{Amount, Symbol},
    };

    #[test]
    fn test_basket() -> Result<()> {
        let assert_tolerance = dec!(0.00001);

        // Define some assets - they will stay
        let asset_btc = Arc::new(Asset::new("BTC".into()));
        let asset_eth = Arc::new(Asset::new("ETH".into()));
        let asset_sol = Arc::new(Asset::new("SOL".into()));

        // Define basket - it will be consumed when we create Basket
        let basket_definition = BasketDefinition::try_new([
            AssetWeight::new(asset_btc.clone(), dec!(0.25)),
            AssetWeight::new(asset_eth.clone(), dec!(0.75)),
        ])?;

        println!("basket_definition = {}", basket_definition);

        // Tell reference prices for assets for in basket quantities computation
        let individual_prices: HashMap<Symbol, Amount> = [
            (asset_btc.name.clone(), dec!(50000.0)),
            (asset_eth.name.clone(), dec!(6000.0)),
        ]
        .into();

        // Set target price for computing actual quantites for the basket
        let target_price = dec!(10000.0);

        // Create actual basket consuming the definition
        let basket = Basket::new_with_prices(basket_definition, &individual_prices, target_price)?;

        println!("basket = {}", basket);

        assert_decimal_approx_eq!(
            basket.get_current_price(&individual_prices)?,
            target_price,
            assert_tolerance
        );

        // Tell reference prices for assets for in basket quantities computation
        let individual_prices_updated: HashMap<Symbol, Amount> = [
            (asset_btc.name.clone(), dec!(55000.0)),
            (asset_eth.name.clone(), dec!(6250.0)),
            (asset_sol.name.clone(), dec!(250.0)),
        ]
        .into();

        // Get current price of all the assets in the basket
        let target_price_updated = basket.get_current_price(&individual_prices_updated)?;

        // Create new rebalanced basket consuming old basket and new prices
        let updated_basket = Basket::new_with_prices(
            basket.try_into()?,
            &individual_prices_updated,
            target_price_updated,
        )?;

        println!("updated_basket = {}", updated_basket);

        // Assert that quantites were calcualted so that total price of assets matches target price after update
        assert_decimal_approx_eq!(
            updated_basket.get_current_price(&individual_prices_updated)?,
            target_price_updated,
            assert_tolerance
        );

        let mut weights_updated: Vec<AssetWeight> = updated_basket.try_into()?;

        weights_updated.push(AssetWeight::new(asset_sol, dec!(0.15)));

        let basket_definition_updated = BasketDefinition::try_new(weights_updated)?;

        println!("basket_definition_updated = {}", basket_definition_updated);

        let updated_basket_2 = Basket::new_with_prices(
            basket_definition_updated,
            &individual_prices_updated,
            target_price_updated,
        )?;

        println!("updated_basket_2 = {}", updated_basket_2);

        assert_decimal_approx_eq!(
            updated_basket_2.get_current_price(&individual_prices_updated)?,
            target_price_updated,
            assert_tolerance
        );

        Ok(())
    }
}
