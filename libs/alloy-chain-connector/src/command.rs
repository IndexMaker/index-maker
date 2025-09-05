use std::sync::Arc;

use alloy_primitives::{Bytes, B256, U256};
use chrono::{DateTime, Utc};
use index_core::index::basket::Basket;
use symm_core::core::{
    bits::{Address, Amount, Symbol},
    functional::SingleObserver,
};

pub enum BasicCommand {
    BalanceOf {
        account: Address,
        observer: SingleObserver<Amount>,
    },
    Transfer {
        receipient: Address,
        amount: Amount,
        observer: SingleObserver<Amount>,
    },
}

pub enum IssuerCommand {
    SetSolverWeights {
        basket: Arc<Basket>,
        price: Amount,
        observer: SingleObserver<Amount>,
    },
    MintIndex {
        receipient: Address,
        amount: Amount,
        seq_num_execution_report: U256,
        observer: SingleObserver<Amount>,
    },
    BurnIndex {
        sender: Address,
        amount: Amount,
        seq_num_new_order_single: U256,
        observer: SingleObserver<Amount>,
    },
    Withdraw {
        receipient: Address,
        amount: Amount,
        execution_report: Bytes,
        observer: SingleObserver<Amount>,
    },
}

pub enum CustodyCommand {
    AddressToCustody {
        amount: Amount,
        observer: SingleObserver<Amount>,
    },
    CustodyToAddress {
        destination: Address,
        amount: Amount,
        observer: SingleObserver<Amount>,
    },
    GetCustodyBalances {
        observer: SingleObserver<Amount>,
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
}

pub struct Command {
    pub command: CommandVariant,
    pub error_observer: SingleObserver<eyre::Report>,
}
