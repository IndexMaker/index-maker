use std::{collections::HashMap, sync::Arc};

use alloy_primitives::{bytes, U256};
use chrono::{DateTime, Utc};
use eyre::{eyre, OptionExt, Result};
use index_core::{
    blockchain::chain_connector::{ChainConnector, ChainNotification},
    index::basket::Basket,
};
use otc_custody::{custody_client::CustodyClient, index::index::IndexInstance};
use parking_lot::RwLock as AtomicLock;
use safe_math::safe;
use symm_core::{
    core::{
        bits::{Address, Amount, Symbol},
        decimal_ext::DecimalExt,
        functional::{
            IntoObservableSingleArc, IntoObservableSingleVTable, NotificationHandlerOnce,
            OneShotSingleObserver, SingleObserver,
        },
    },
    market_data::exchange_rates::ExchangeRates,
};
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver};

use crate::{
    arbiter::Arbiter,
    chain_connector_sender::RealChainConnectorSender,
    command::{Command, CommandVariant, IssuerCommand},
    credentials::Credentials,
    session::SessionBaggage,
    sessions::Sessions,
    subaccounts::SubAccounts,
};

#[derive(Clone)]
pub struct GasFeeCalculator {
    exchange_rates: Arc<dyn ExchangeRates + Send + Sync + 'static>,
    base: Symbol,
    quote: Symbol,
}

impl GasFeeCalculator {
    pub fn new(
        exchange_rates: Arc<dyn ExchangeRates + Send + Sync + 'static>,
        quote: Symbol,
    ) -> Self {
        Self {
            exchange_rates,
            base: Symbol::from("ETH"),
            quote,
        }
    }

    pub fn compute_amount(&self, base_amount: Amount) -> eyre::Result<Amount> {
        let rate = self
            .exchange_rates
            .get_exchange_rate(self.base.clone(), self.quote.clone())?;

        let quote_amount =
            safe!(rate * base_amount).ok_or_eyre("Failed to compute quote amount")?;

        Ok(quote_amount)
    }
}

pub struct RealChainConnector {
    observer: Arc<AtomicLock<SingleObserver<ChainNotification>>>,
    subaccounts: Arc<AtomicLock<SubAccounts>>,
    subaccount_rx: Option<UnboundedReceiver<Credentials>>,
    sessions: Arc<AtomicLock<Sessions>>,
    baggage: SessionBaggage,
    gas_fee_calculator: GasFeeCalculator,
    arbiter: Arbiter,
    issuers: HashMap<u32, Arc<RealChainConnectorSender>>,
}

impl RealChainConnector {
    pub fn new(gas_fee_calculator: GasFeeCalculator) -> Self {
        let (subaccount_tx, subaccount_rx) = unbounded_channel();
        Self {
            observer: Arc::new(AtomicLock::new(SingleObserver::new())),
            subaccounts: Arc::new(AtomicLock::new(SubAccounts::new(subaccount_tx))),
            subaccount_rx: Some(subaccount_rx),
            sessions: Arc::new(AtomicLock::new(Sessions::new())),
            baggage: SessionBaggage {
                custody_clients: Arc::new(AtomicLock::new(HashMap::new())),
                indexes_by_symbol: Arc::new(AtomicLock::new(HashMap::new())),
                indexes_by_address: Arc::new(AtomicLock::new(HashMap::new())),
            },
            gas_fee_calculator,
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
            self.baggage.clone(),
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
        self.baggage
            .custody_clients
            .write()
            .insert(custody_id, custody_client)
            .is_none()
            .then_some(())
            .ok_or_eyre("Duplicate custody client")
    }

    pub fn add_index(&self, index: Arc<IndexInstance>) -> Result<()> {
        let symbol = Symbol::from(index.get_symbol());
        let address = index.get_index_address();

        self.baggage
            .indexes_by_symbol
            .write()
            .insert(symbol, index.clone())
            .is_none()
            .then_some(())
            .ok_or_eyre("Duplicate index")?;

        self.baggage
            .indexes_by_address
            .write()
            .insert(*address, index)
            .is_none()
            .then_some(())
            .ok_or_eyre("Duplicate index")
    }

    pub fn create_sender(
        &self,
        accounts: impl IntoIterator<Item = String>,
    ) -> Arc<RealChainConnectorSender> {
        Arc::new(RealChainConnectorSender::new(
            self.sessions.clone(),
            accounts,
        ))
    }

    pub fn set_issuer(
        &mut self,
        chain_id: u32,
        sender: Arc<RealChainConnectorSender>,
    ) -> Result<()> {
        self.issuers
            .insert(chain_id, sender)
            .is_none()
            .then_some(())
            .ok_or_eyre("Duplicate issuer entry")
    }

    pub(crate) fn send_command_to_issuer(
        &self,
        chain_id: u32,
        symbol: Symbol,
        command: IssuerCommand,
        error_observer: OneShotSingleObserver<eyre::Report>,
    ) -> Result<()> {
        let sender = self.issuers.get(&chain_id).ok_or_eyre("Issuer not found")?;
        sender.send_command(Command {
            command: CommandVariant::Issuer { symbol, command },
            error_observer,
        })
    }
}

impl ChainConnector for RealChainConnector {
    fn solver_weights_set(&self, symbol: Symbol, basket: Arc<Basket>) {
        let chain_id = 0;

        let command = IssuerCommand::SetSolverWeights {
            basket,
            price: Amount::ZERO,
            observer: OneShotSingleObserver::new(),
        };

        let error_observer = OneShotSingleObserver::new_with_fn(move |err| {
            tracing::warn!("Failed to call solver weights set: {:?}", err);
        });

        if let Err(err) = self.send_command_to_issuer(chain_id, symbol, command, error_observer) {
            tracing::warn!("Failed to send command to issuer: {:?}", err);
        }
    }

    fn mint_index(
        &self,
        chain_id: u32,
        symbol: Symbol,
        quantity: Amount,
        receipient: Address,
        seq_num: U256,
        execution_price: Amount,
        execution_time: DateTime<Utc>,
    ) {
        let _ = execution_price;
        let _ = execution_time;

        let symbol_clone = symbol.clone();
        let gas_fee_calculator = self.gas_fee_calculator.clone();

        let error_observer = OneShotSingleObserver::new_with_fn(move |err| {
            tracing::warn!("Failed to mint index: {:?}", err);
        });

        let command = IssuerCommand::MintIndex {
            receipient,
            amount: quantity,
            seq_num_execution_report: seq_num,
            observer: OneShotSingleObserver::new_with_fn(move |gas_used| {
                let gas_amount_usdc = match gas_fee_calculator.compute_amount(gas_used) {
                    Ok(x) => x,
                    Err(err) => {
                        tracing::warn!("❗️ Failed to compute gas fee: {:?}", err);
                        Amount::ZERO
                    }
                };
                tracing::info!(
                    "💳 Index Minted: {} {} to: {} fee: {} ",
                    quantity,
                    symbol_clone,
                    receipient,
                    gas_amount_usdc
                );
                // TODO: Propagate to Solver
                //self.observer.read().publish_single(ChainNotification::IndexMinted);
            }),
        };

        if let Err(err) = self.send_command_to_issuer(chain_id, symbol, command, error_observer) {
            tracing::warn!("Failed to send command to issuer: {:?}", err);
        }
    }

    fn burn_index(&self, chain_id: u32, symbol: Symbol, quantity: Amount, sender: Address) {
        let _ = symbol;

        let command = IssuerCommand::BurnIndex {
            amount: quantity,
            sender,
            seq_num_new_order_single: U256::ZERO,
            observer: OneShotSingleObserver::new(),
        };

        let error_observer = OneShotSingleObserver::new_with_fn(move |err| {
            tracing::warn!("Failed to burn index: {:?}", err);
        });

        if let Err(err) = self.send_command_to_issuer(chain_id, symbol, command, error_observer) {
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

        let error_observer = OneShotSingleObserver::new_with_fn(move |err| {
            tracing::warn!("Failed to withdraw: {:?}", err);
        });

        let command = IssuerCommand::Withdraw {
            amount,
            receipient,
            execution_report: bytes!("0x0000"),
            observer: OneShotSingleObserver::new(),
        };

        if let Err(err) = self.send_command_to_issuer(chain_id, symbol, command, error_observer) {
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
