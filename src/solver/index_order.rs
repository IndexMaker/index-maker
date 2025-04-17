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

#[derive(Clone)]
pub struct IndexOrderUpdate {
    /// On-chain wallet address
    ///
    /// An address of subsequent buyer / seller. Users may have exchange token
    /// on-chain, and ownership is split.
    pub address: Address,

    /// ID of the update assigned by the user (<- FIX)
    pub client_order_id: ClientOrderId,

    /// An ID of the corresponding payment
    /// 
    /// Note: In case of Buy it is an ID allocated for the payment that will
    /// come from them to cover for the transaction. And in case of Sell, there
    /// will be ID allocated to identify the payment that we will make to them
    /// in relationship with this update.
    pub payment_id: PaymentId,

    /// Buy or Sell
    pub side: Side,

    /// Limit price
    pub price: Amount,

    /// Price max deviation %-age (as fraction) threshold
    pub price_threshold: Amount,

    /// Quantity of an index to buy or sell
    pub original_quantity: Amount,

    /// Quantity remaining after applying matching update
    pub remaining_quantity: Amount,

    /// Fee for updating the order
    pub update_fee: Amount,
    
    /// Timestamp
    pub timestamp: DateTime<Utc>,
}

/// An order to buy index
pub struct IndexOrder {
    /// On-chain wallet address
    ///
    /// An address of the first user who had the index created. First buyer.
    pub original_address: Address,

    /// ID of the Index Order assigned by the user (<- FIX)
    pub original_client_order_id: ClientOrderId,

    /// An index symbol
    pub symbol: Symbol,

    /// Time of when this order was created
    pub created_timestamp: DateTime<Utc>,

    /// Time of the last update to this order
    pub last_update_timestamp: DateTime<Utc>,

    /// Order updates
    pub order_updates: VecDeque<IndexOrderUpdate>,

    /// Past order updates
    pub closed_updates: VecDeque<IndexOrderUpdate>,
}
