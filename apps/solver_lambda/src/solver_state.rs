use std::sync::Arc;

use index_core::index::basket::Basket;
use serde::{Deserialize, Serialize};
use symm_core::core::bits::{BatchOrderId, OrderId, PaymentId, Symbol};

#[derive(Serialize, Deserialize)]
pub struct IndexDefinition {
    pub symbol: Symbol,
    pub basket: Arc<Basket>,
}

#[derive(Serialize, Deserialize)]
pub struct SolverState {
    pub order_ids: Vec<OrderId>,
    pub batch_ids: Vec<BatchOrderId>,
    pub payment_ids: Vec<PaymentId>,

    pub indexes: Vec<IndexDefinition>,

    pub index_orders: Option<serde_json::Value>,
    pub invoices: Option<serde_json::Value>,
    pub collateral: Option<serde_json::Value>,
    pub inventory: Option<serde_json::Value>,
    pub batch: Option<serde_json::Value>,
    pub solver: Option<serde_json::Value>,
}
