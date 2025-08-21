use std::{collections::HashMap, sync::Arc};

use alloy_primitives::{bytes, U256};
use chrono::{DateTime, Utc};
use eyre::{eyre, OptionExt, Result};
use index_core::{
    blockchain::chain_connector::{ChainConnector, ChainNotification},
    index::basket::Basket,
};
use parking_lot::RwLock as AtomicLock;
use symm_core::core::{
    bits::{Address, Amount, Symbol},
    functional::{
        IntoObservableSingleArc, IntoObservableSingleVTable, NotificationHandlerOnce,
        SingleObserver,
    },
};
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver};

use crate::{
    arbiter::Arbiter,
    command::{Command, CommandVariant, IssuerCommand},
    credentials::Credentials,
    sessions::Sessions,
    subaccounts::SubAccounts,
};

pub struct RealChainConnector {
    observer: Arc<AtomicLock<SingleObserver<ChainNotification>>>,
    subaccounts: Arc<AtomicLock<SubAccounts>>,
    subaccount_rx: Option<UnboundedReceiver<Credentials>>,
    sessions: Arc<AtomicLock<Sessions>>,
    arbiter: Arbiter,
    issuers: HashMap<u32, (String, Address)>,
}

impl RealChainConnector {
    pub fn new() -> Self {
        let (subaccount_tx, subaccount_rx) = unbounded_channel();
        Self {
            observer: Arc::new(AtomicLock::new(SingleObserver::new())),
            subaccounts: Arc::new(AtomicLock::new(SubAccounts::new(subaccount_tx))),
            subaccount_rx: Some(subaccount_rx),
            sessions: Arc::new(AtomicLock::new(Sessions::new())),
            arbiter: Arbiter::new(),
            issuers: HashMap::new(),
        }
    }

    pub fn start(&mut self) -> Result<()> {
        let subaccount_rx = self
            .subaccount_rx
            .take()
            .ok_or_eyre("Subaccount receiver unavailable")?;

        self.arbiter.start(
            self.subaccounts.clone(),
            subaccount_rx,
            self.sessions.clone(),
            self.observer.clone(),
        );

        Ok(())
    }

    pub async fn stop(&mut self) -> Result<()> {
        let subaccount_rx = self
            .arbiter
            .stop()
            .await
            .map_err(|err| eyre!("Error stopping arbiter {}", err))?;

        self.subaccount_rx
            .replace(subaccount_rx)
            .is_none()
            .then_some(())
            .ok_or_eyre("Invalid state of subaccount receiver")?;

        Ok(())
    }

    pub fn logon(&mut self, subaccounts: impl IntoIterator<Item = Credentials>) -> Result<()> {
        self.subaccounts.write().logon(subaccounts)
    }

    pub(crate) fn send_command_to_session(
        &self,
        account_name: &str,
        command: Command,
    ) -> Result<()> {
        let sessions_read = self.sessions.read();
        let issuer = sessions_read
            .get_session(account_name)
            .ok_or_eyre("Session not found")?;

        issuer.send_command(command)
    }

    pub fn set_issuer(
        &mut self,
        chain_id: u32,
        account_name: String,
        contract_address: Address,
    ) -> Result<()> {
        self.issuers
            .insert(chain_id, (account_name, contract_address))
            .is_none()
            .then_some(())
            .ok_or_eyre("Duplicate issuer entry")
    }

    pub(crate) fn send_command_to_issuer(
        &self,
        chain_id: u32,
        command: IssuerCommand,
    ) -> Result<()> {
        let (account_name, contract_address) =
            self.issuers.get(&chain_id).ok_or_eyre("Issuer not found")?;

        self.send_command_to_session(
            account_name,
            Command {
                command: CommandVariant::Issuer(command),
                contract_address: *contract_address,
                error_observer: SingleObserver::new(),
            },
        )
    }
}

impl ChainConnector for RealChainConnector {
    fn solver_weights_set(&self, symbol: Symbol, basket: Arc<Basket>) {
        let chain_id = 0;
        let _ = symbol;

        let command = IssuerCommand::SetSolverWeights {
            timestamp: Utc::now(),
            basket,
            price: Amount::ZERO,
            observer: SingleObserver::new(),
        };

        if let Err(err) = self.send_command_to_issuer(chain_id, command) {
            tracing::warn!("Failed to send command to issuer: {:?}", err);
        }
    }

    fn mint_index(
        &self,
        chain_id: u32,
        symbol: Symbol,
        quantity: Amount,
        receipient: Address,
        execution_price: Amount,
        execution_time: DateTime<Utc>,
    ) {
        let _ = symbol;
        let _ = quantity;
        let _ = execution_price;
        let _ = execution_time;

        let command = IssuerCommand::MintIndex {
            target: receipient,
            amount: quantity,
            seq_num_execution_report: U256::ZERO,
            observer: SingleObserver::new(),
        };

        if let Err(err) = self.send_command_to_issuer(chain_id, command) {
            tracing::warn!("Failed to send command to issuer: {:?}", err);
        }
    }

    fn burn_index(&self, chain_id: u32, symbol: Symbol, quantity: Amount, receipient: Address) {
        let _ = chain_id;
        let _ = symbol;
        let _ = quantity;

        let command = IssuerCommand::BurnIndex {
            amount: quantity,
            target: receipient,
            seq_num_new_order_single: U256::ZERO,
            observer: SingleObserver::new(),
        };

        if let Err(err) = self.send_command_to_issuer(chain_id, command) {
            tracing::warn!("Failed to send command to issuer: {:?}", err);
        }
    }

    fn withdraw(
        &self,
        chain_id: u32,
        receipient: Address,
        amount: Amount,
        execution_price: Amount,
        execution_time: DateTime<Utc>,
    ) {
        let _ = chain_id;
        let _ = amount;
        let _ = execution_price;
        let _ = execution_time;

        let command = IssuerCommand::Withdraw {
            amount,
            receipient,
            execution_report: bytes!("0x0000"),
            observer: SingleObserver::new(),
        };

        if let Err(err) = self.send_command_to_issuer(chain_id, command) {
            tracing::warn!("Failed to send command to issuer: {:?}", err);
        }
    }
}

impl IntoObservableSingleArc<ChainNotification> for RealChainConnector {
    fn get_single_observer_arc(&mut self) -> &Arc<AtomicLock<SingleObserver<ChainNotification>>> {
        &self.observer
    }
}

impl IntoObservableSingleVTable<ChainNotification> for RealChainConnector {
    fn set_observer(&mut self, observer: Box<dyn NotificationHandlerOnce<ChainNotification>>) {
        self.observer.write().set_observer(observer);
    }
}
