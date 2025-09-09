use std::{
    collections::{self, HashMap},
    sync::Arc,
};

use eyre::{eyre, OptionExt, Result};
use parking_lot::RwLock as AtomicLock;
use safe_math::safe;
use symm_core::{
    core::{
        self,
        bits::{self, Amount, SingleOrder, Symbol},
        decimal_ext::DecimalExt,
        functional::{
            self, IntoObservableSingleArc, IntoObservableSingleVTable, NotificationHandlerOnce,
            OneShotSingleObserver, SingleObserver,
        },
    },
    market_data::exchange_rates::ExchangeRates,
    order_sender::order_connector::{OrderConnector, OrderConnectorNotification, SessionId},
};
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver};

use crate::{
    arbiter::Arbiter, command::Command, credentials::Credentials, sessions::Sessions,
    subaccounts::SubAccounts,
};

#[derive(Clone)]
pub struct BinanceFeeCalculator {
    exchange_rates: Arc<dyn ExchangeRates + Send + Sync + 'static>,
    quote: Symbol,
}

impl BinanceFeeCalculator {
    pub fn new(
        exchange_rates: Arc<dyn ExchangeRates + Send + Sync + 'static>,
        quote: Symbol,
    ) -> Self {
        Self {
            exchange_rates,
            quote,
        }
    }

    pub fn compute_amount(&self, base_amount: Amount, base_symbol: Symbol) -> eyre::Result<Amount> {
        let rate = self
            .exchange_rates
            .get_exchange_rate(base_symbol, self.quote.clone())?;

        let quote_amount =
            safe!(rate * base_amount).ok_or_eyre("Failed to compute quote amount")?;

        Ok(quote_amount)
    }
}

pub struct BinanceOrderSending {
    observer: Arc<AtomicLock<SingleObserver<OrderConnectorNotification>>>,
    subaccounts: Arc<AtomicLock<SubAccounts>>,
    subaccount_rx: Option<UnboundedReceiver<Credentials>>,
    sessions: Arc<AtomicLock<Sessions>>,
    fee_calculator: BinanceFeeCalculator,
    arbiter: Arbiter,
}

impl BinanceOrderSending {
    pub fn new(fee_calculator: BinanceFeeCalculator) -> Self {
        let (subaccount_tx, subaccount_rx) = unbounded_channel();
        Self {
            observer: Arc::new(AtomicLock::new(SingleObserver::new())),
            subaccounts: Arc::new(AtomicLock::new(SubAccounts::new(subaccount_tx))),
            subaccount_rx: Some(subaccount_rx),
            sessions: Arc::new(AtomicLock::new(Sessions::new())),
            fee_calculator,
            arbiter: Arbiter::new(),
        }
    }

    pub fn start(&mut self, symbols: Vec<Symbol>) -> Result<()> {
        let subaccount_rx = self
            .subaccount_rx
            .take()
            .ok_or_eyre("Subaccount receiver unavailable")?;

        self.arbiter.start(
            self.subaccounts.clone(),
            subaccount_rx,
            symbols,
            self.fee_calculator.clone(),
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
}

impl OrderConnector for BinanceOrderSending {
    fn send_order(&mut self, session_id: SessionId, order: &Arc<SingleOrder>) -> Result<()> {
        tracing::debug!("Send to: {} command: {:#?}", session_id, &*order);
        let sessions = self.sessions.read();
        let session = sessions
            .get_session(&session_id)
            .ok_or_else(|| eyre!("Cannot find session: {}", session_id))?;

        session
            .send_command(Command::NewOrder(order.clone()))
            .map_err(|_| eyre!("Failed to send new order"))
    }

    fn get_balances(
        &self,
        session_id: SessionId,
        observer: OneShotSingleObserver<HashMap<Symbol, Amount>>,
    ) -> Result<()> {
        tracing::debug!("Send to: {} command: Get Balances", session_id);
        let sessions = self.sessions.read();
        let session = sessions
            .get_session(&session_id)
            .ok_or_else(|| eyre!("Cannot find session: {}", session_id))?;

        session
            .send_command(Command::GetBalances(observer))
            .map_err(|_| eyre!("Failed to send new order"))
    }
}

impl IntoObservableSingleArc<OrderConnectorNotification> for BinanceOrderSending {
    fn get_single_observer_arc(
        &mut self,
    ) -> &Arc<AtomicLock<SingleObserver<OrderConnectorNotification>>> {
        &self.observer
    }
}

impl IntoObservableSingleVTable<OrderConnectorNotification> for BinanceOrderSending {
    fn set_observer(
        &mut self,
        observer: Box<dyn NotificationHandlerOnce<OrderConnectorNotification>>,
    ) {
        self.observer.write().set_observer(observer);
    }
}
