use std::sync::{Arc, RwLock, Weak};

use alloy_primitives::U256;
use crossbeam::channel::bounded;
use eyre::{eyre, OptionExt};
use index_core::collateral::collateral_router::CollateralDesignation;
use symm_core::core::{
    bits::{Address, Amount, Symbol},
    functional::SingleObserver,
};

use crate::{
    chain_connector::RealChainConnector,
    command::{Command, CommandVariant, CustodyCommand},
};

pub struct OTCCustodyCollateralDesignation {
    designation_type: Symbol,
    name: Symbol,
    collateral_symbol: Symbol,
    full_name: Symbol,
    chain_connector_weak: Weak<RwLock<RealChainConnector>>,
    account_name: String,
    chain_id: u32,
    contract_address: Address,
    token_address: Address,
    custody_id: U256,
}

impl OTCCustodyCollateralDesignation {
    pub fn new(
        designation_type: Symbol,
        name: Symbol,
        collateral_symbol: Symbol,
        chain_connector: Arc<RwLock<RealChainConnector>>,
        account_name: String,
        chain_id: u32,
        contract_address: Address,
        token_address: Address,
        custody_id: U256,
    ) -> Self {
        let full_name = Symbol::from(format!(
            "{}:{}:{}",
            designation_type, name, collateral_symbol
        ));
        Self {
            designation_type,
            name,
            collateral_symbol,
            full_name,
            chain_connector_weak: Arc::downgrade(&chain_connector),
            account_name,
            chain_id,
            contract_address,
            token_address,
            custody_id,
        }
    }

    pub fn get_chain_id(&self) -> u32 {
        self.chain_id
    }

    pub fn get_contract_address(&self) -> Address {
        self.contract_address
    }

    pub fn get_token_address(&self) -> Address {
        self.token_address
    }

    pub fn custody_to_address(
        &self,
        destination: Address,
        amount: Amount,
        observer: SingleObserver<Amount>,
        error_observer: SingleObserver<eyre::Report>,
    ) -> eyre::Result<()> {
        let chain_connector = self
            .chain_connector_weak
            .upgrade()
            .ok_or_eyre("Chain connector is gone")?;

        let chain_connector = chain_connector
            .read()
            .map_err(|err| eyre!("Failed to read chain connector: {:?}", err))?;

        let command = Command {
            command: CommandVariant::Custody(CustodyCommand::CustodyToAddress {
                token: self.token_address,
                destination,
                amount,
                observer,
            }),
            error_observer,
        };

        chain_connector.send_command_to_session(&self.account_name, command)?;

        Ok(())
    }

    pub fn address_to_custody(
        &self,
        source: Address,
        amount: Amount,
        observer: SingleObserver<Amount>,
        error_observer: SingleObserver<eyre::Report>,
    ) -> eyre::Result<()> {
        let _ = source;

        let chain_connector = self
            .chain_connector_weak
            .upgrade()
            .ok_or_eyre("Chain connector is gone")?;

        let chain_connector = chain_connector
            .read()
            .map_err(|err| eyre!("Failed to read chain connector: {:?}", err))?;

        let command = Command {
            command: CommandVariant::Custody(CustodyCommand::AddressToCustody {
                custody_id: self.custody_id,
                token: self.token_address,
                amount,
                observer,
            }),
            error_observer,
        };

        chain_connector.send_command_to_session(&self.account_name, command)?;

        Ok(())
    }
}

impl CollateralDesignation for OTCCustodyCollateralDesignation {
    fn get_type(&self) -> Symbol {
        self.designation_type.clone()
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
        /////
        // NOTE:
        // This implementation is merely a concept and needs further tought.
        // We currently don't use get_balance() in solver, but we plan to
        // use it at some stage, then we will change this to suit the use case.
        //
        // TODO:
        // - Function should return result
        // - Function is blocking but timeout is hard-coded atm.
        // ? Consider using observer callback instead

        let chain_connector = self
            .chain_connector_weak
            .upgrade()
            .expect("Failed to upgrade weak chain connector");

        let chain_connector = chain_connector.write().expect("Failed to obtain lock");

        let (balance_tx, balance_rx) = bounded(1);
        let (err_tx, err_rx) = bounded(1);

        let command = Command {
            command: CommandVariant::Custody(CustodyCommand::GetCustodyBalances {
                custody_id: U256::ZERO,
                token: self.token_address,
                observer: SingleObserver::new_from(balance_tx),
            }),
            error_observer: SingleObserver::new_from(err_tx),
        };

        chain_connector
            .send_command_to_session(&self.account_name, command)
            .expect("Failed to send get balance");

        let timeout = std::time::Duration::from_secs(3);

        crossbeam::select! {
            recv(balance_rx) -> res => {
                let balance_u256 = res.expect("Failed to obtain balance");
                let _ = balance_u256;
            }
            recv(err_rx) -> res => {
                let res = res.expect("Failed to obtain error");
                panic!("{:?}", res);
            }
            default(timeout) => {
                panic!("Failed to get balances before timeout");
            }
        }

        todo!()
    }
}
