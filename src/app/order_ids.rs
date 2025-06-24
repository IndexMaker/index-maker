use chrono::Utc;
use symm_core::core::bits::{BatchOrderId, OrderId, PaymentId};

use crate::solver::solver::OrderIdProvider;

pub struct TimestampOrderIds {}

impl OrderIdProvider for TimestampOrderIds {
    fn next_order_id(&mut self) -> OrderId {
        OrderId::from(format!("O-{}", Utc::now().timestamp_millis()))
    }

    fn next_batch_order_id(&mut self) -> BatchOrderId {
        BatchOrderId::from(format!("B-{}", Utc::now().timestamp_millis()))
    }

    fn next_payment_id(&mut self) -> PaymentId {
        PaymentId::from(format!("P-{}", Utc::now().timestamp_millis()))
    }
}


