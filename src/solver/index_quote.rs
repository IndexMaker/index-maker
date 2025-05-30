use chrono::{DateTime, Utc};
use parking_lot::RwLock;

use crate::core::bits::{Address, Amount, ClientQuoteId, Side, Symbol};

pub struct IndexQuoteData {
}

/// a quote on index
pub struct IndexQuote {
    /// Chain ID
    pub chain_id: u32,

    /// On-chain wallet address
    pub address: Address,

    /// Cliend order ID
    pub client_quote_id: ClientQuoteId,

    /// An index symbol
    pub symbol: Symbol,

    /// Buy or Sell
    pub side: Side,

    /// Collateral amount to quote for
    pub collateral_amount: Amount,
    
    /// Quantity of an index possible to obtain
    pub quantity_possible: Amount,

    /// Time quote was created
    pub created_timestamp: DateTime<Utc>,

    /// Time quote was responded (updated)
    pub last_update_timestamp: DateTime<Utc>,
}
