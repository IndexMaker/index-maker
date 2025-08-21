use std::sync::Arc;

use alloy_primitives::{Bytes, U256};
use chrono::{DateTime, Utc};
use index_core::index::basket::Basket;
use symm_core::core::{
    bits::{Address, Amount},
    functional::SingleObserver,
};

pub enum BasicCommand {
    BalanceOf {
        account: Address,
        observer: SingleObserver<Amount>,
    },
    Transfer {
        to: Address,
        amount: Amount,
        observer: SingleObserver<Amount>,
    },
}

pub enum IssuerCommand {
    SetSolverWeights {
        basket: Arc<Basket>,
        price: Amount,
        timestamp: DateTime<Utc>,
        observer: SingleObserver<Amount>,
    },
    MintIndex {
        target: Address,
        amount: Amount,
        seq_num_execution_report: U256,
        observer: SingleObserver<Amount>,
    },
    BurnIndex {
        target: Address,
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
        custody_id: U256,
        token: Address,
        amount: Amount,
        observer: SingleObserver<Amount>,
    },
    CustodyToAddress {
        token: Address,
        destination: Address,
        amount: Amount,
        observer: SingleObserver<Amount>,
    },
    GetCustodyBalances {
        custody_id: U256,
        token: Address,
        observer: SingleObserver<Amount>,
    },
}

pub enum CommandVariant {
    Basic(BasicCommand),
    Issuer(IssuerCommand),
    Custody(CustodyCommand),
}

pub struct Command {
    pub command: CommandVariant,
    pub error_observer: SingleObserver<eyre::Report>,
}
