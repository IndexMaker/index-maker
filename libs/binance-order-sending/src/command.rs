use std::{collections::HashMap, sync::Arc};

use symm_core::core::{bits::{Amount, SingleOrder, Symbol}, functional::OneShotSingleObserver};

#[derive(Debug)]
pub enum Command {
    EnableTrading(bool),
    NewOrder(Arc<SingleOrder>),
    GetExchangeInfo(Vec<Symbol>),
    GetBalances(OneShotSingleObserver<HashMap<Symbol, Amount>>),
}
