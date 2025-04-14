use chrono::{DateTime, Utc};

pub type Symbol = string_cache::DefaultAtom; // asset or market name
pub type Amount = rust_decimal::Decimal; // price, quantity, value, or rate
pub type Address = alloy::primitives::Address; // address (EVM)

// add things like (de)serialization of Amount from string (...when required)

#[derive(Default)]
pub struct OrderId();

#[derive(Default)]
pub struct ClientOrderId();

#[derive(Default)]
pub struct  LotId();

#[derive(Default)]
pub struct Lot {
    pub original_order_id: OrderId, // internal order ID, multiple per IndexOrder
    pub original_client_order_id: ClientOrderId, // order ID from FIX message, one per IndexOrder
    pub lot_id: LotId,
    pub remaining_quantity: Amount,
    pub original_quantity: Amount,
    pub original_price_in_usdc: Amount,
    pub created_timestamp: DateTime<Utc>,
    pub updated_timestamp:  DateTime<Utc>
}

#[derive(Default)]
pub struct Order {

}