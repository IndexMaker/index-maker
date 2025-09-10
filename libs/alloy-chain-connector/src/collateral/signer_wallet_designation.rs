use std::sync::Arc;

use index_core::collateral::collateral_router::CollateralDesignation;
use symm_core::core::{
    bits::{Address, Amount, Symbol},
    functional::OneShotSingleObserver,
};

use crate::{
    chain_connector_sender::RealChainConnectorSender,
    command::{BasicCommand, Command, CommandVariant},
};

pub struct SignerWalletCollateralDesignation {
    sender: Arc<RealChainConnectorSender>,
    designation_type: Symbol,
    name: Symbol,
    collateral_symbol: Symbol,
    full_name: Symbol,
    chain_id: u32,
    address: Address,
    token_address: Address,
}

impl SignerWalletCollateralDesignation {
    pub fn new(
        sender: Arc<RealChainConnectorSender>,
        designation_type: Symbol,
        name: Symbol,
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
            sender,
            designation_type,
            name,
            collateral_symbol,
            full_name,
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
        observer: OneShotSingleObserver<Amount>,
        error_observer: OneShotSingleObserver<eyre::Report>,
    ) -> eyre::Result<()> {
        let _ = amount;
        let command = Command {
            command: CommandVariant::Basic {
                contract_address: self.token_address,
                command: BasicCommand::Transfer {
                    receipient,
                    amount,
                    observer,
                },
            },
            error_observer,
        };

        self.sender.send_command(command)
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
