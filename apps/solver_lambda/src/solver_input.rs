use chrono::{DateTime, Utc};
use index_core::{
    blockchain::chain_connector::ChainNotification,
    collateral::collateral_router::CollateralRouterEvent,
};
use index_maker::server::server::ServerEvent;
use serde::{Deserialize, Serialize};
use symm_core::{
    market_data::market_data_connector::MarketDataEvent,
    order_sender::order_connector::OrderConnectorNotification,
};

use crate::solver_state::SolverState;

#[derive(Serialize, Deserialize)]
pub struct SolverInput {
    pub market_data_events: Vec<MarketDataEvent>,
    pub order_events: Vec<OrderConnectorNotification>,
    pub server_events: Vec<ServerEvent>,
    pub chain_events: Vec<ChainNotification>,
    pub router_events: Vec<CollateralRouterEvent>,
    pub state: SolverState,
    pub timestamp: DateTime<Utc>,
}
