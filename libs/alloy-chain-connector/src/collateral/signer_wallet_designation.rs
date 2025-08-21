use std::sync::{RwLock, Weak};

use eyre::{eyre, OptionExt};

use index_core::collateral::collateral_router::CollateralDesignation;
use symm_core::core::{
    bits::{Address, Amount, Symbol},
    functional::SingleObserver,
};

use crate::{
    chain_connector::RealChainConnector,
    command::{BasicCommand, Command, CommandVariant},
};

pub struct SignerWalletCollateralDesignation {
    designation_type: Symbol,
    name: Symbol,
    collateral_symbol: Symbol,
    full_name: Symbol,
    chain_connector_weak: Weak<RwLock<RealChainConnector>>,
    account_name: String,
    chain_id: u32,
    address: Address,
    token_address: Address,
}

impl SignerWalletCollateralDesignation {
    pub fn new(
        designation_type: Symbol,
        name: Symbol,
        chain_connector_weak: Weak<RwLock<RealChainConnector>>,
        account_name: String,
        collateral_symbol: Symbol,
        chain_id: u32,
        address: Address,
        token_address: Address,
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
            chain_connector_weak,
            account_name,
            chain_id,
            address,
            token_address,
        }
    }

    pub fn get_chain_id(&self) -> u32 {
        self.chain_id
    }

    pub fn get_address(&self) -> Address {
        self.address
    }

    pub fn get_token_address(&self) -> Address {
        self.token_address
    }

    pub fn transfer_to_account(
        &self,
        receipient: Address,
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

        let _ = amount;
        let command = Command {
            contract_address: self.token_address,
            command: CommandVariant::Basic(BasicCommand::Transfer {
                to: receipient,
                amount,
                observer,
            }),
            error_observer,
        };

        chain_connector.send_command_to_session(&self.account_name, command)?;

        Ok(())
    }
}

impl CollateralDesignation for SignerWalletCollateralDesignation {
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
        todo!()
    }
}
