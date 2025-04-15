use std::fmt::Display;

use chrono::{DateTime, Utc};

pub type Symbol = string_cache::DefaultAtom; // asset or market name
pub type Amount = rust_decimal::Decimal; // price, quantity, value, or rate
pub type Address = alloy::primitives::Address; // address (EVM)

// add things like (de)serialization of Amount from string (...when required)

#[derive(Default, Hash, Eq, PartialEq, Clone, Debug)]
pub struct OrderId(pub String);

impl Display for OrderId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "OrderID({})", self.0)
    }
}

#[derive(Default, Hash, Eq, PartialEq, Clone, Debug)]
pub struct ClientOrderId(pub String);

impl Display for ClientOrderId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "ClientOrderID({})", self.0)
    }
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

#[derive(Default)]
pub struct LotId();

#[derive(Default)]
pub struct Lot {
    pub original_order_id: OrderId, // internal order ID, multiple per IndexOrder
    pub original_client_order_id: ClientOrderId, // order ID from FIX message, one per IndexOrder
    pub lot_id: LotId,              // internal lot ID, assigned for this lot
    pub remaining_quantity: Amount,
    pub original_quantity: Amount,
    pub original_price_in_usdc: Amount,
    pub created_timestamp: DateTime<Utc>,
    pub updated_timestamp: DateTime<Utc>,
}

#[derive(Clone, Copy)]
pub enum Side {
    Buy,
    Sell,
}

pub struct Order {
    pub order_id: OrderId,              // internal ID we assign to order
    pub client_order_id: ClientOrderId, // order ID from FIX message, one per IndexOrder
    pub symbol: Symbol,                 // asset we want to buy or sell on exchange (-> Binance)
    pub side: Side,
    pub price: Amount,
    pub quantity: Amount,
}
