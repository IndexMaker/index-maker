use chrono::{DateTime, Utc};

use crate::core::bits::{Address, Amount, IndexOrderId, Side, Symbol};

/// an order to buy/sell index
pub struct IndexOrder {
    /// On-chain wallet address
    pub address: Address,

    /// Amount paid into custody
    pub amount_paid_in: Amount,

    /// ID of the Index Order assigned by the user (<- FIX)
    pub index_order_id: IndexOrderId,

    /// An index symbol
    pub symbol: Symbol,

    /// Buy or Sell
    pub side: Side,

    /// Limit price
    pub price: Amount,

    /// Price max deviation %-age (as fraction) threshold
    pub price_threshold: Amount,

    /// Quantity of an index to buy or sell
    pub quantity: Amount,

    /// Time of when this order was created
    pub created_timestamp: DateTime<Utc>,

}
