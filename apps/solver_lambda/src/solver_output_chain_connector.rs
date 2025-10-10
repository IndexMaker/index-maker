use std::{collections::VecDeque, sync::Arc};

use chrono::{DateTime, Utc};
use index_core::{
    blockchain::chain_connector::{ChainConnector, ChainNotification},
    index::basket::Basket,
};
use parking_lot::RwLock;
use symm_core::core::{
    bits::{Address, Amount, Symbol},
    functional::{IntoObservableSingleVTable, NotificationHandlerOnce},
};

use crate::solver_output::ChainCommand;

pub struct SolverOutputChainConnector {
    pub commands: RwLock<VecDeque<ChainCommand>>,
}

impl SolverOutputChainConnector {
    pub fn new() -> Self {
        Self {
            commands: RwLock::new(VecDeque::new()),
        }
    }
}

impl ChainConnector for SolverOutputChainConnector {
    fn poll_once(&self, chain_id: u32, address: Address, symbol: Symbol) {
        self.commands.write().push_back(ChainCommand::PollOnce {
            chain_id,
            address,
            symbol,
        });
    }

    fn solver_weights_set(&self, symbol: Symbol, basket: Arc<Basket>) {
        self.commands
            .write()
            .push_back(ChainCommand::SolverWeightsSet { symbol, basket });
    }

    fn mint_index(
        &self,
        chain_id: u32,
        symbol: Symbol,
        quantity: Amount,
        receipient: Address,
        seq_num: alloy::primitives::U256,
        execution_price: Amount,
        execution_time: DateTime<Utc>,
    ) {
        self.commands.write().push_back(ChainCommand::MintIndex {
            chain_id,
            symbol,
            quantity,
            receipient,
            seq_num,
            execution_price,
            execution_time,
        });
    }

    fn burn_index(&self, chain_id: u32, symbol: Symbol, quantity: Amount, receipient: Address) {
        self.commands.write().push_back(ChainCommand::BurnIndex {
            chain_id,
            symbol,
            quantity,
            receipient,
        });
    }

    fn withdraw(
        &self,
        chain_id: u32,
        receipient: Address,
        amount: Amount,
        execution_price: Amount,
        execution_time: DateTime<Utc>,
    ) {
        self.commands.write().push_back(ChainCommand::Withdraw {
            chain_id,
            receipient,
            amount,
            execution_price,
            execution_time,
        });
    }
}

impl IntoObservableSingleVTable<ChainNotification> for SolverOutputChainConnector {
    fn set_observer(&mut self, _observer: Box<dyn NotificationHandlerOnce<ChainNotification>>) {
        unimplemented!()
    }
}
