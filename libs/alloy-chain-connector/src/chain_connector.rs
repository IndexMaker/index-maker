use std::{collections::HashMap, sync::Arc};

use alloy_primitives::{bytes, B256, U256};
use chrono::{DateTime, Utc};
use eyre::{eyre, OptionExt, Result};
use index_core::{
    blockchain::chain_connector::{ChainConnector, ChainNotification},
    index::basket::Basket,
};
use otc_custody::{custody_client::CustodyClient, index::index::IndexInstance};
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
    custody_clients: Arc<AtomicLock<HashMap<B256, CustodyClient>>>,
    indexes: Arc<AtomicLock<HashMap<Symbol, Arc<IndexInstance>>>>,
    arbiter: Arbiter,
    issuers: HashMap<u32, String>,
}

impl RealChainConnector {
    pub fn new() -> Self {
        let (subaccount_tx, subaccount_rx) = unbounded_channel();
        Self {
            observer: Arc::new(AtomicLock::new(SingleObserver::new())),
            subaccounts: Arc::new(AtomicLock::new(SubAccounts::new(subaccount_tx))),
            subaccount_rx: Some(subaccount_rx),
            sessions: Arc::new(AtomicLock::new(Sessions::new())),
            custody_clients: Arc::new(AtomicLock::new(HashMap::new())),
            indexes: Arc::new(AtomicLock::new(HashMap::new())),
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
            self.custody_clients.clone(),
            self.indexes.clone(),
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

    pub fn add_custody_client(&self, custody_client: CustodyClient) -> Result<()> {
        let custody_id = custody_client.get_custody_id();
        self.custody_clients
            .write()
            .insert(custody_id, custody_client)
            .is_none()
            .then_some(())
            .ok_or_eyre("Duplicate custody client")
    }

    pub fn add_index(&self, index: Arc<IndexInstance>) -> Result<()> {
        let symbol = Symbol::from(index.get_symbol());
        self.indexes
            .write()
            .insert(symbol, index)
            .is_none()
            .then_some(())
            .ok_or_eyre("Duplicate index")
    }

    pub fn set_issuer(&mut self, chain_id: u32, account_name: String) -> Result<()> {
        self.issuers
            .insert(chain_id, account_name)
            .is_none()
            .then_some(())
            .ok_or_eyre("Duplicate issuer entry")
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

    pub(crate) fn send_command_to_issuer(
        &self,
        chain_id: u32,
        symbol: Symbol,
        command: IssuerCommand,
    ) -> Result<()> {
        let account_name = self.issuers.get(&chain_id).ok_or_eyre("Issuer not found")?;

        self.send_command_to_session(
            account_name,
            Command {
                command: CommandVariant::Issuer { symbol, command },
                error_observer: SingleObserver::new(),
            },
        )
    }
}

impl ChainConnector for RealChainConnector {
    fn solver_weights_set(&self, symbol: Symbol, basket: Arc<Basket>) {
        let chain_id = 0;

        let command = IssuerCommand::SetSolverWeights {
            basket,
            price: Amount::ZERO,
            observer: SingleObserver::new(),
        };

        if let Err(err) = self.send_command_to_issuer(chain_id, symbol, command) {
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
        let _ = execution_price;
        let _ = execution_time;

        let command = IssuerCommand::MintIndex {
            receipient,
            amount: quantity,
            seq_num_execution_report: U256::ZERO,
            observer: SingleObserver::new(),
        };

        if let Err(err) = self.send_command_to_issuer(chain_id, symbol, command) {
            tracing::warn!("Failed to send command to issuer: {:?}", err);
        }
    }

    fn burn_index(&self, chain_id: u32, symbol: Symbol, quantity: Amount, sender: Address) {
        let _ = symbol;

        let command = IssuerCommand::BurnIndex {
            amount: quantity,
            sender,
            seq_num_new_order_single: U256::ZERO,
            observer: SingleObserver::new(),
        };

        if let Err(err) = self.send_command_to_issuer(chain_id, symbol, command) {
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
        let _ = execution_price;
        let _ = execution_time;
        let symbol = Symbol::from("");

        let command = IssuerCommand::Withdraw {
            amount,
            receipient,
            execution_report: bytes!("0x0000"),
            observer: SingleObserver::new(),
        };

        if let Err(err) = self.send_command_to_issuer(chain_id, symbol, command) {
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
