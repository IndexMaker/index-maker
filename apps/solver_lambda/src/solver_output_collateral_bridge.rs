use std::{collections::VecDeque, sync::Arc};

use index_core::collateral::collateral_router::{
    CollateralBridge, CollateralDesignation, CollateralRouterEvent,
};
use itertools::Itertools;
use parking_lot::RwLock;
use symm_core::core::{
    bits::{Address, Amount, ClientOrderId, Symbol},
    functional::{IntoObservableSingleVTableRef, NotificationHandlerOnce},
};

use crate::solver_output::BridgeCommand;

pub struct SolverOutputCollateralDesignation {
    full_name: Symbol,
    type_: Symbol,
    name: Symbol,
    collateral_symbol: Symbol,
}

pub struct SolverOutputCollateralBridge {
    pub source: Arc<SolverOutputCollateralDesignation>,
    pub destination: Arc<SolverOutputCollateralDesignation>,
    pub commands: RwLock<VecDeque<BridgeCommand>>,
}

impl SolverOutputCollateralDesignation {
    pub fn new(full_name: &str) -> Self {
        let (type_, name, collateral_symbol) =
            full_name.split(':').map_into().collect_tuple().unwrap();

        Self {
            full_name: Symbol::from(full_name),
            type_,
            name,
            collateral_symbol,
        }
    }
}

impl CollateralDesignation for SolverOutputCollateralDesignation {
    fn get_type(&self) -> Symbol {
        self.type_.clone()
    }

    fn get_name(&self) -> Symbol {
        self.name.clone()
    }

    fn get_collateral_symbol(&self) -> Symbol {
        self.collateral_symbol.clone()
    }

    fn get_full_name(&self) -> Symbol {
        self.full_name.clone()
    }

    fn get_balance(&self) -> Amount {
        unimplemented!()
    }
}

impl SolverOutputCollateralBridge {
    pub fn new(
        source: Arc<SolverOutputCollateralDesignation>,
        destination: Arc<SolverOutputCollateralDesignation>,
    ) -> Self {
        Self {
            source,
            destination,
            commands: RwLock::new(VecDeque::new()),
        }
    }
}

impl IntoObservableSingleVTableRef<CollateralRouterEvent> for SolverOutputCollateralBridge {
    fn set_observer(&self, _observer: Box<dyn NotificationHandlerOnce<CollateralRouterEvent>>) {
        unimplemented!()
    }
}

impl CollateralBridge for SolverOutputCollateralBridge {
    fn get_source(&self) -> Arc<dyn CollateralDesignation> {
        self.source.clone()
    }

    fn get_destination(&self) -> Arc<dyn CollateralDesignation> {
        self.destination.clone()
    }

    fn transfer_funds(
        &self,
        chain_id: u32,
        address: Address,
        client_order_id: ClientOrderId,
        route_from: Symbol,
        route_to: Symbol,
        amount: Amount,
        cumulative_fee: Amount,
    ) -> eyre::Result<()> {
        self.commands
            .write()
            .push_back(BridgeCommand::TransferFunds {
                chain_id,
                address,
                client_order_id,
                source: self.source.get_full_name(),
                destination: self.destination.get_full_name(),
                route_from,
                route_to,
                amount,
                cumulative_fee,
            });
        Ok(())
    }
}
