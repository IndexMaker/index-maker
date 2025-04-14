use crate::core::bits::Amount;

/// implement price level order book allowing to inspect market depth
pub struct OrderBook {}

impl OrderBook {
    pub fn get_liquidity(&self, _price: &Amount, _threshold: Amount) -> Amount {
        Amount::default()
    }
}
