use std::collections::VecDeque;

use chrono::{DateTime, Utc};

use crate::core::bits::{Address, Amount, ClientOrderId, PaymentId, Side, Symbol};

pub enum PaymentDirection {
    /// Credit the giver: they gave us money, that increases our liability
    /// towards them. If they credit their account with us, we owe them index.
    Credit,
    /// Debit the receiver: they receive money, that decreases our liability
    /// towards them. If they debit their account, we get their index.
    Debit
}

pub struct Payment {
    /// On-chain wallet address
    pub address: Address,

    /// An ID of this payment
    pub payment_id: PaymentId,

    /// Credit or Debit
    pub direction: PaymentDirection,

    /// Amount paid from(to) custody to(from) user wallet
    pub amount: Amount,
}

pub struct IndexOrderUpdate {
    /// On-chain wallet address
    ///
    /// An address of subsequent buyer / seller. Users may have exchange token
    /// on-chain, and ownership is split.
    pub address: Address,

    /// ID of the update assigned by the user (<- FIX)
    pub client_order_id: ClientOrderId,

    /// An ID of the corresponding payment
    pub payment_id: PaymentId,

    /// Buy or Sell
    pub side: Side,

    /// Limit price
    pub price: Amount,

    /// Price max deviation %-age (as fraction) threshold
    pub price_threshold: Amount,

    /// Quantity of an index to buy or sell
    pub quantity: Amount,
}

/// an order to buy/sell index
pub struct IndexOrder {
    /// On-chain wallet address
    ///
    /// An address of the first user who had the index created. First buyer.
    pub original_address: Address,

    /// Amount paid into custody
    pub original_amount_paid_in: Amount,

    /// ID of the Index Order assigned by the user (<- FIX)
    pub original_client_order_id: ClientOrderId,

    /// An ID of the corresponding payment
    pub original_payment_id: PaymentId,

    /// An index symbol
    pub symbol: Symbol,

    /// Limit price
    pub original_price: Amount,

    /// Price max deviation %-age (as fraction) threshold
    pub original_price_threshold: Amount,

    /// Quantity of an index to buy or sell
    pub original_quantity: Amount,

    /// Time of when this order was created
    pub created_timestamp: DateTime<Utc>,

    /// Time of the last update to this order
    pub last_update_timestamp: DateTime<Utc>,

    /// A queue of pending updates to this Index Order
    pub pending_updates: VecDeque<IndexOrderUpdate>,

    /// A queue of updates applied to this Index Order
    pub applied_updates: VecDeque<IndexOrderUpdate>,
}
