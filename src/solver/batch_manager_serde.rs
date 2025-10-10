use std::{collections::HashMap, sync::Arc};

use alloy::rpc::types::trace::tracerequest::TraceCallRequest;
use eyre::OptionExt;
use index_core::index::basket::Basket;
use itertools::Itertools;
use serde::{Deserialize, Serialize};
use symm_core::core::{
    bits::{Address, Amount, BatchOrderId, ClientOrderId, Side, Symbol},
    telemetry::TracingData,
};

use crate::solver::solver::{
    EngagedSolverOrders, EngagedSolverOrdersSide, FetchSolverOrder, SolverOrderEngagement,
};

#[derive(Serialize, Deserialize)]
pub struct StoredSolverOrderEngagement {
    pub asset_contribution_fractions: HashMap<Symbol, Amount>,
    pub asset_quantity_contributions: HashMap<Symbol, Amount>,
    pub chain_id: u32,
    pub address: Address,
    pub client_order_id: ClientOrderId,
    pub symbol: Symbol,
    pub engaged_side: Side,
    pub engaged_collateral: Amount,
    pub new_engaged_collateral: Amount,
    pub engaged_quantity: Amount,
    pub engaged_price: Amount,
    pub filled_quantity: Amount,
}

#[derive(Serialize, Deserialize)]
pub struct StoredEngagedSolverOrdersSide {
    pub asset_price_limits: HashMap<Symbol, Amount>,
    pub asset_quantities: HashMap<Symbol, Amount>,
    pub engaged_orders: Vec<StoredSolverOrderEngagement>,
}

#[derive(Serialize, Deserialize)]
pub struct StoredEngagedSolverOrders {
    pub batch_order_id: BatchOrderId,
    pub engaged_buys: StoredEngagedSolverOrdersSide,
}

impl StoredSolverOrderEngagement {
    pub fn new(engagement: &SolverOrderEngagement) -> Self {
        Self {
            asset_contribution_fractions: engagement.asset_contribution_fractions.clone(),
            asset_quantity_contributions: engagement.asset_quantity_contributions.clone(),
            chain_id: engagement.chain_id,
            address: engagement.address,
            client_order_id: engagement.client_order_id.clone(),
            symbol: engagement.symbol.clone(),
            engaged_side: engagement.engaged_side,
            engaged_collateral: engagement.engaged_collateral,
            new_engaged_collateral: engagement.new_engaged_collateral,
            engaged_quantity: engagement.engaged_quantity,
            engaged_price: engagement.engaged_price,
            filled_quantity: engagement.filled_quantity,
        }
    }

    pub fn to_live(self, parent: &dyn FetchSolverOrder) -> eyre::Result<SolverOrderEngagement> {
        let index_order = parent
            .fetch_client_order(self.chain_id, &self.address, &self.client_order_id)
            .ok_or_eyre("Failed to fetch index order")?;

        let basket = parent
            .fetch_basket(&self.symbol)
            .ok_or_eyre("Failed to fetch basket")?;

        Ok(SolverOrderEngagement {
            index_order,
            asset_contribution_fractions: self.asset_contribution_fractions,
            asset_quantity_contributions: self.asset_quantity_contributions,
            chain_id: self.chain_id,
            address: self.address,
            client_order_id: self.client_order_id,
            symbol: self.symbol,
            basket,
            engaged_side: self.engaged_side,
            engaged_collateral: self.engaged_collateral,
            new_engaged_collateral: self.new_engaged_collateral,
            engaged_quantity: self.engaged_quantity,
            engaged_price: self.engaged_price,
            filled_quantity: self.filled_quantity,
        })
    }
}

impl StoredEngagedSolverOrdersSide {
    pub fn new(engaged: &EngagedSolverOrdersSide) -> Self {
        Self {
            asset_price_limits: engaged.asset_price_limits.clone(),
            asset_quantities: engaged.asset_quantities.clone(),
            engaged_orders: engaged
                .engaged_orders
                .iter()
                .map(|v| StoredSolverOrderEngagement::new(v))
                .collect_vec(),
        }
    }

    pub fn to_live(self, parent: &dyn FetchSolverOrder) -> eyre::Result<EngagedSolverOrdersSide> {
        let (engaged_orders, failed): (Vec<_>, Vec<_>) = self
            .engaged_orders
            .into_iter()
            .map(|x| x.to_live(parent))
            .partition_result();

        if !failed.is_empty() {
            Err(eyre::eyre!("Failed to load engaged orders: {:?}", failed))?;
        }

        Ok(EngagedSolverOrdersSide {
            asset_price_limits: self.asset_price_limits,
            asset_quantities: self.asset_quantities,
            engaged_orders,
        })
    }
}

impl StoredEngagedSolverOrders {
    pub fn new(engagement: &EngagedSolverOrders) -> Self {
        Self {
            batch_order_id: engagement.batch_order_id.clone(),
            engaged_buys: StoredEngagedSolverOrdersSide::new(&engagement.engaged_buys),
        }
    }

    pub fn to_live(self, parent: &dyn FetchSolverOrder) -> eyre::Result<EngagedSolverOrders> {
        let engaged_buys = self.engaged_buys.to_live(parent)?;
        Ok(EngagedSolverOrders {
            batch_order_id: self.batch_order_id,
            engaged_buys,
            trace_data: TracingData::default(),
        })
    }
}
