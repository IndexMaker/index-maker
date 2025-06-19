use chrono::{DateTime, Utc};

use crate::core::bits::{Address, Amount, ClientQuoteId, Side, Symbol};

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

impl IndexQuote {
    pub fn new(
        chain_id: u32,
        address: Address,
        client_quote_id: ClientQuoteId,
        symbol: Symbol,
        side: Side,
        collateral_amount: Amount,
        timestamp: DateTime<Utc>,
    ) -> Self {
        Self {
            chain_id,
            address,
            client_quote_id,
            symbol,
            side,
            collateral_amount,
            quantity_possible: Amount::ZERO,
            created_timestamp: timestamp,
            last_update_timestamp: timestamp,
        }
    }
}
