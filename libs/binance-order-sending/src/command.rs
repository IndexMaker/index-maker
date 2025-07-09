use std::sync::Arc;

use symm_core::core::bits::{SingleOrder, Symbol};

#[derive(Debug)]
pub enum Command {
    EnableTrading(bool),
    NewOrder(Arc<SingleOrder>),
    GetExchangeInfo(Vec<Symbol>),
}
