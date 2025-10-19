use alloy::primitives::U256;
use chrono::{DateTime, Utc};
use index_core::index::basket::Basket;
use index_maker::server::server::ServerResponse;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use symm_core::{
    core::bits::{Address, Amount, ClientOrderId, SingleOrder, Symbol},
    order_sender::order_connector::SessionId,
};

use crate::solver_state::SolverState;

#[derive(Serialize, Deserialize)]
pub enum ChainCommand {
    PollOnce {
        chain_id: u32,
        address: Address,
        symbol: Symbol,
    },
    SolverWeightsSet {
        symbol: Symbol,
        basket: Arc<Basket>,
    },
    MintIndex {
        chain_id: u32,
        symbol: Symbol,
        quantity: Amount,
        receipient: Address,
        seq_num: U256,
        execution_price: Amount,
        execution_time: DateTime<Utc>,
    },
    BurnIndex {
        chain_id: u32,
        symbol: Symbol,
        quantity: Amount,
        receipient: Address,
    },
    Withdraw {
        chain_id: u32,
        receipient: Address,
        amount: Amount,
        execution_price: Amount,
        execution_time: DateTime<Utc>,
    },
}

#[derive(Serialize, Deserialize)]
pub enum BridgeCommand {
    TransferFunds {
        chain_id: u32,
        address: Address,
        client_order_id: ClientOrderId,
        source: Symbol,
        destination: Symbol,
        route_from: Symbol,
        route_to: Symbol,
        amount: Amount,
        cumulative_fee: Amount,
    },
}

#[derive(Serialize, Deserialize)]
pub struct SolverOutput {
    pub orders: Vec<(SessionId, Arc<SingleOrder>)>,
    pub server_responses: Vec<ServerResponse>,
    pub chain_commands: Vec<ChainCommand>,
    pub bridge_commands: Vec<BridgeCommand>,
    pub state: SolverState,
}
