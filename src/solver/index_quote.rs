use chrono::{DateTime, Utc};

use crate::core::bits::{Address, Amount, Side, Symbol};

/// a quote on index
pub struct IndexQuote {
    /// On-chain wallet address
    pub address: Address,

    /// An index symbol
    pub symbol: Symbol,

    /// Buy or Sell
    pub side: Side,

    /// Collateral amount to quote for
    pub collateral_amount: Amount,

    /// Quantity that collateral can buy
    pub quantity: Amount,

    /// Fees that will be paid if quantity was executed
    pub fees: Amount,

    /// Time quote was created
    pub created_timestamp: DateTime<Utc>,

    /// Time quote was responded (updated)
    pub last_update_timestamp: DateTime<Utc>,
}
