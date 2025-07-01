use std::sync::Arc;

use symm_core::{core::bits::SingleOrder, order_sender::order_connector::SessionId};

#[derive(Debug)]
pub enum Command {
    EnableTrading(bool),
    NewOrder(Arc<SingleOrder>),
    GetExchangeInfo(),
}
