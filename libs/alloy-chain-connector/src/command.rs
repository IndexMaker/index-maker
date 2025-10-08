use std::sync::Arc;

use alloy_primitives::{Bytes, B256, U256};
use index_core::index::basket::Basket;
use symm_core::core::{
    bits::{Address, Amount, Symbol},
    functional::OneShotSingleObserver,
};

pub enum BasicCommand {
    BalanceOf {
        account: Address,
        observer: OneShotSingleObserver<Amount>,
    },
    Transfer {
        receipient: Address,
        amount: Amount,
        observer: OneShotSingleObserver<Amount>,
    },
}

pub enum IssuerCommand {
    SetSolverWeights {
        basket: Arc<Basket>,
        price: Amount,
        observer: OneShotSingleObserver<Amount>,
    },
    MintIndex {
        receipient: Address,
        amount: Amount,
        seq_num_execution_report: U256,
        observer: OneShotSingleObserver<Amount>,
    },
    BurnIndex {
        sender: Address,
        amount: Amount,
        seq_num_new_order_single: U256,
        observer: OneShotSingleObserver<Amount>,
    },
    Withdraw {
        receipient: Address,
        amount: Amount,
        execution_report: Bytes,
        observer: OneShotSingleObserver<Amount>,
    },
}

pub enum CustodyCommand {
    AddressToCustody {
        amount: Amount,
        observer: OneShotSingleObserver<Amount>,
    },
    CustodyToAddress {
        destination: Address,
        amount: Amount,
        observer: OneShotSingleObserver<Amount>,
    },
    GetCustodyBalances {
        observer: OneShotSingleObserver<Amount>,
    },
}

pub enum CommandVariant {
    Basic {
        contract_address: Address,
        command: BasicCommand,
    },
    Issuer {
        symbol: Symbol,
        command: IssuerCommand,
    },
    Custody {
        custody_id: B256,
        token: Address,
        command: CustodyCommand,
    },
    PollIssuerEvent {
        chain_id: u32,
        address: Address,
        symbol: Symbol,
    },
}

pub struct Command {
    pub command: CommandVariant,
    pub error_observer: OneShotSingleObserver<eyre::Report>,
}
