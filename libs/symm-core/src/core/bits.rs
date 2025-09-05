use crate::{core::decimal_ext::DecimalExt, string_id};
use chrono::{DateTime, Utc};
use safe_math::safe;
use serde::{Deserialize, Serialize};

pub type Symbol = string_cache::DefaultAtom; // asset or market name
pub type Amount = rust_decimal::Decimal; // price, quantity, value, or rate
pub type Address = alloy::primitives::Address; // address (EVM)

// add things like (de)serialization of Amount from string (...when required)

#[derive(Clone, Copy, Debug)]
pub enum PriceType {
    BestBid,
    BestAsk,
    LastTrade,
    MidPoint,
    VolumeWeighted,
}

#[derive(Clone, Debug)]
pub struct LastPriceEntry {
    pub sequence_number: u64,
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
        safe!(safe!(self.best_bid_price + self.best_ask_price?) / Amount::TWO)
    }

    pub fn volume_weighted(&self) -> Option<Amount> {
        safe!(
            safe!(
                safe!(self.best_bid_price * self.best_bid_quantity)?
                    + safe!(self.best_ask_price * self.best_ask_quantity)?
            )? / safe!(self.best_bid_quantity + self.best_ask_quantity)?
        )
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

#[derive(Clone, Debug)]
pub struct PricePointEntry {
    pub price: Amount,
    pub quantity: Amount,
}

string_id!(OrderId);
string_id!(BatchOrderId);
string_id!(ClientOrderId);
string_id!(ClientQuoteId);
string_id!(PaymentId);

#[derive(Hash, Eq, PartialEq, Clone, Copy, Serialize, Deserialize, Debug)]
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

impl<T> From<T> for Side
where
    T: AsRef<str>,
{
    fn from(value: T) -> Self {
        match value.as_ref() {
            "buy" => Side::Buy,
            "Buy" => Side::Buy,
            "BUY" => Side::Buy,
            "bid" => Side::Buy,
            "Bid" => Side::Buy,
            "BID" => Side::Buy,
            "B" => Side::Buy,
            "b" => Side::Buy,
            "sell" => Side::Sell,
            "Sell" => Side::Sell,
            "SELL" => Side::Sell,
            "S" => Side::Sell,
            "s" => Side::Sell,
            "ask" => Side::Sell,
            "Ask" => Side::Sell,
            "ASK" => Side::Sell,
            "A" => Side::Sell,
            "a" => Side::Sell,
            _ => panic!("Invalid side"),
        }
    }
}

/// Single leg of a Batch Order
#[derive(Debug)]
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
#[derive(Debug)]
pub struct BatchOrder {
    /// An order ID from FIX message, one per IndexOrder
    pub batch_order_id: BatchOrderId,

    /// List of order legs, one per asset (Buys & Sells are combined)
    pub asset_orders: Vec<AssetOrder>,

    /// Time of when this order was created
    pub created_timestamp: DateTime<Utc>,
}

#[derive(Debug)]
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
