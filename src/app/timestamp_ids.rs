use symm_core::core::bits::{BatchOrderId, OrderId, PaymentId};

use crate::{app::timestamp_ids::util::make_timestamp_id, solver::solver::OrderIdProvider};

pub struct TimestampOrderIds {}

pub mod util {
    use chrono::Utc;

    pub fn make_timestamp_id<T>(prefix: &str) -> T
    where
        T: From<String>,
    {
        T::from(format!("{}{}", prefix, Utc::now().timestamp_millis()))
    }
}

impl OrderIdProvider for TimestampOrderIds {
    fn next_order_id(&mut self) -> OrderId {
        make_timestamp_id("O-")
    }

    fn next_batch_order_id(&mut self) -> BatchOrderId {
        make_timestamp_id("B-")
    }

    fn next_payment_id(&mut self) -> PaymentId {
        make_timestamp_id("P-")
    }
}
