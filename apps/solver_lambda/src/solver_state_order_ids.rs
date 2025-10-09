use std::collections::VecDeque;

use index_maker::solver::solver::OrderIdProvider;
use symm_core::core::bits::{BatchOrderId, OrderId, PaymentId};

pub struct SolverStateOrderIdProvider {
    pub order_ids: VecDeque<OrderId>,
    pub batch_order_ids: VecDeque<BatchOrderId>,
    pub payment_ids: VecDeque<PaymentId>,
}

impl OrderIdProvider for SolverStateOrderIdProvider {
    fn next_order_id(&mut self) -> OrderId {
        self.order_ids.pop_front().expect("No more Order Ids")
    }

    fn next_batch_order_id(&mut self) -> BatchOrderId {
        self.batch_order_ids
            .pop_front()
            .expect("No more BatchOrder Ids")
    }

    fn next_payment_id(&mut self) -> PaymentId {
        self.payment_ids
            .pop_front()
            .expect("No more PaymentIds Ids")
    }
}
