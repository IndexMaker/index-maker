use std::fmt::Display;

use chrono::{DateTime, Utc};

pub type Symbol = string_cache::DefaultAtom; // asset or market name
pub type Amount = rust_decimal::Decimal; // price, quantity, value, or rate
pub type Address = alloy::primitives::Address; // address (EVM)

// add things like (de)serialization of Amount from string (...when required)

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


/// IndexOrderId is intended to be used for index orders (<- FIX)
/// 
/// User will put ID on their Index Order.
#[derive(Default, Hash, Eq, PartialEq, Clone, Debug)]
pub struct IndexOrderId(pub String);

impl Display for IndexOrderId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "IndexOrderId({})", self.0)
    }
}


/// Lot is what you get in a single execution, so Lot Id is same as execution Id and comes from exchange (<- Binance)
///
/// From exchange perspective execution Id is the Id of the *action*, which is to execute an order.
/// However from our perspective, when our order is executed what we receive is a *lot* of an asset,
/// for us it is not execution that matters, but the actual quantity of asset we received in one
/// transaction, and that we call *lot*. We manage lots and not executions. We handle executions by managing lots.
/// When we get an execution of the Buy order, then we open a lot, and when we get an execution of the Sell order
/// we match that new lot against the one we opened for Buy order. Lots form a stack (LIFO) or queue (FIFO).
/// We always match incoming lot from Sell transation against current stack/queue. Note that we said Buy opens a lot
/// and Sell closes one or more lots. When short-selling is supported these can be inverted, but we don't support
/// short-selling.
///
#[derive(Default, Hash, Eq, PartialEq, Clone, Debug)]
pub struct LotId(pub String);

impl Display for LotId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "ClientOrderID({})", self.0)
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
            Self::Sell => Self::Buy
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

pub struct LotTransaction {
    /// ID of the closing order that was executed
    pub order_id: OrderId,

    /// ID of the associated batch order
    pub batch_order_id: BatchOrderId,

    /// ID of the matching lot, essentially ID of the transaction, which closed portion of the lot
    pub matched_lot_id: LotId,

    /// Quantity from matching transaction, it can be same as quantity on the transaction or less,
    /// because matching transaction could have more quantity than available in this lot, and had
    /// to be matched against more than one lot.
    pub quantity_closed: Amount,

    /// Price on the matching transaction
    pub closing_price: Amount,

    /// Fee paid
    pub closing_fee: Amount,

    /// Time of the closing transaction
    pub closing_timestamp: DateTime<Utc>,
}

pub struct Lot {
    /// ID of the order that was executed, and caused to open this lot
    pub original_order_id: OrderId,

    /// ID of the associated batch order
    pub original_batch_order_id: BatchOrderId,

    /// ID of this lot, essentially ID of the transaction, which opened this lot
    pub lot_id: LotId,

    /// An asset we received
    pub symbol: Symbol,

    /// Buy (Long) or Sell (Short)
    /// 
    /// Added for completness & correctness. We should always be on the Long side. 
    /// 
    pub side: Side,

    /// Price on transaction that opened this lot
    pub original_price: Amount,

    /// Quantity on transaction that opened this lot
    pub original_quantity: Amount,

    /// Fee paid
    pub original_fee: Amount,

    /// Quantity remaining in this lot after most recent transation
    pub remaining_quantity: Amount,

    /// Time of the first transaction that opened this lot
    pub created_timestamp: DateTime<Utc>,

    /// Time of the last transaction matched against this lot
    pub last_update_timestamp: DateTime<Utc>,

    /// All the transactions that were matched against this lot, and these transactions
    /// closed some portion of this lot.
    pub lot_transactions: Vec<LotTransaction>,
}
