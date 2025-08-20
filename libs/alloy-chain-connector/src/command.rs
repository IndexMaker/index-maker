use alloy_primitives::{Bytes, FixedBytes, U256};
use symm_core::core::{bits::Address, functional::SingleObserver};

use crate::contracts::VerificationData;

pub enum IssuerCommand {
    SetSolverWeights {
        timestamp: U256,
        weights: Bytes,
        price: U256,
    },
    MintIndex {
        target: Address,
        amount: U256,
        seq_num_execution_report: U256,
    },
    BurnIndex {
        amount: U256,
        target: Address,
        seq_num_new_order_single: U256,
    },
    Withdraw {
        amount: U256,
        to: Address,
        verification_data: VerificationData,
        execution_report: Bytes,
    },
}

pub enum CustodyCommand {
    AddressToCustody {
        id: FixedBytes<32>,
        token: Address,
        amount: U256,
        observer: SingleObserver<U256>,
    },
    CustodyToAddress {
        token: Address,
        destination: Address,
        amount: U256,
        verification_data: VerificationData,
        observer: SingleObserver<U256>,
    },
    GetCustodyBalances {
        id: FixedBytes<32>,
        token: Address,
        observer: SingleObserver<U256>,
    },
}

pub enum CommandVariant {
    Issuer(IssuerCommand),
    Custody(CustodyCommand),
}

pub struct Command {
    pub command: CommandVariant,
    pub error_observer: SingleObserver<eyre::Report>,
}
