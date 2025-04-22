use std::fmt::Display;

use chrono::{DateTime, Utc};
use eyre::{eyre, Result};

pub type Symbol = string_cache::DefaultAtom; // asset or market name
pub type Amount = rust_decimal::Decimal; // price, quantity, value, or rate
pub type Address = alloy::primitives::Address; // address (EVM)

// add things like (de)serialization of Amount from string (...when required)

pub fn add_amounts(a: Amount, b: Amount) -> Result<Amount> {
    a.checked_add(b).ok_or(eyre!("Math overflow"))
}

pub fn sub_amounts(a: Amount, b: Amount) -> Result<Amount> {
    a.checked_sub(b).ok_or(eyre!("Math overflow"))
}

/// Add amount to optional amount
pub fn add_amount_to_optional(a: Option<Amount>, b: Amount) -> Result<Option<Amount>> {
    let c = match a {
        None => Some(b),
        Some(a) => Some(a.checked_add(b).ok_or(eyre!("Match overflow"))?),
    };
    Ok(c)
}

/// Add two optional amounts
pub fn add_optional_amounts(a: Option<Amount>, b: Option<Amount>) -> Result<Option<Amount>> {
    let c = match (a, b) {
        (None, None) => None,
        (None, Some(b)) => Some(b),
        (Some(a), None) => Some(a),
        (Some(a), Some(b)) => Some(a.checked_add(b).ok_or(eyre!("Match overflow"))?),
    };
    Ok(c)
}

#[derive(Clone, Copy)]
pub enum PriceType {
    BestBid,
    BestAsk,
    LastTrade,
    MidPoint,
    VolumeWeighted,
}

#[derive(Clone)]
pub struct LastPriceEntry {
    pub best_bid_price: Option<Amount>,
    pub best_ask_price: Option<Amount>,
    pub best_bid_quantity: Amount,
    pub best_ask_quantity: Amount,
    pub last_trade_price: Option<Amount>,
    pub last_trade_quantity: Amount,
    //...could also add EMAs here
}

impl LastPriceEntry {
    pub fn mid_point(&self) -> Option<Amount> {
        self.best_bid_price?
            .checked_add(self.best_ask_price?)?
            .checked_div(Amount::TWO)
    }

    pub fn volume_weighted(&self) -> Option<Amount> {
        self.best_bid_price?
            .checked_mul(self.best_bid_quantity)?
            .checked_add(self.best_ask_price?.checked_mul(self.best_ask_quantity)?)?
            .checked_div(self.best_bid_quantity.checked_add(self.best_ask_quantity)?)
    }

    pub fn get_price(&self, price_type: PriceType) -> Option<Amount> {
        match price_type {
            PriceType::BestBid => self.best_bid_price,
            PriceType::BestAsk => self.best_ask_price,
            PriceType::LastTrade => self.last_trade_price,
            PriceType::MidPoint => self.mid_point(),
            PriceType::VolumeWeighted => self.volume_weighted(),
        }
    }
}

#[derive(Clone)]
pub struct PricePointEntry {
    pub price: Amount,
    pub quantity: Amount,
}

/// OrderId is intended to be used for exchange orders (-> Binance)
///
/// This is an ID for an individual order produced from order batch.
#[derive(Default, Hash, Eq, PartialEq, Clone, Debug)]
pub struct OrderId(pub String);

impl Display for OrderId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "OrderId({})", self.0)
    }
}

/// BatchOrderId is intended to be used internally (<- Solver)
///
/// Solver will produce order batches to be taken from market.
#[derive(Default, Hash, Eq, PartialEq, Clone, Debug)]
pub struct BatchOrderId(pub String);

impl Display for BatchOrderId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "BatchOrderId({})", self.0)
    }
}

/// ClientOrderId is intended to be used for index orders and quote requests (<- FIX)
///
/// User will put ID on their requests.
#[derive(Default, Hash, Eq, PartialEq, Clone, Debug)]
pub struct ClientOrderId(pub String);

impl Display for ClientOrderId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "ClientOrderId({})", self.0)
    }
}

/// PaymentId is intended to be used for payments (<-> Blockchain)
///
/// On-chain transactions will produce this ID. It is a confirmation of the payment
/// either from then to us, or from us to them.
#[derive(Default, Hash, Eq, PartialEq, Clone, Debug)]
pub struct PaymentId(pub String);

impl Display for PaymentId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "PaymentId({})", self.0)
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Side {
    Buy,
    Sell,
}

impl Side {
    pub fn opposite_side(&self) -> Side {
        match self {
            Self::Buy => Self::Sell,
            Self::Sell => Self::Buy,
        }
    }
}

/// Single leg of a Batch Order
pub struct AssetOrder {
    /// An internal ID we assign to order
    pub order_id: OrderId,

    /// An asset we want to buy or sell on exchange (-> Binance)
    pub symbol: Symbol,

    /// Buy or Sell
    pub side: Side,

    /// Limit price not to cross
    pub price: Amount,

    /// Maximum quantity to order
    pub quantity: Amount,
}

/// An order multiple legs (Buy & Sell of multiple assets)
///
/// Solver -> produces Batch Orders, sends them into -> InventoryManager, which then
/// matches individial Asset Orders x against Lots, and then remaining unmatched
/// quantites of Asset Orders are sent as Single Orders into -> Order Tracker, which
/// then gets them send to exchange using Order Connector (-> Binance).
///
pub struct BatchOrder {
    /// An order ID from FIX message, one per IndexOrder
    pub batch_order_id: BatchOrderId,

    /// List of order legs, one per asset (Buys & Sells are combined)
    pub asset_orders: Vec<AssetOrder>,

    /// Time of when this order was created
    pub created_timestamp: DateTime<Utc>,
}

pub struct SingleOrder {
    /// An internal ID we assign to order
    pub order_id: OrderId,

    /// An order ID from FIX message, one per IndexOrder
    pub batch_order_id: BatchOrderId,

    /// An asset we want to buy or sell on exchange (-> Binance)
    pub symbol: Symbol,

    /// Buy or Sell
    pub side: Side,

    /// Limit price not to cross
    pub price: Amount,

    /// Maximum quantity to order
    pub quantity: Amount,

    /// Time of when this order was created
    pub created_timestamp: DateTime<Utc>,
}
