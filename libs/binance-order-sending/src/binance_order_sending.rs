use std::sync::Arc;

use binance_spot_connector_rust::http::Credentials;
use eyre::{eyre, OptionExt, Result};
use index_maker::{
    core::{
        bits::SingleOrder,
        functional::{IntoObservableSingleArc, SingleObserver},
    },
    order_sender::order_connector::{OrderConnector, OrderConnectorNotification},
};
use parking_lot::RwLock as AtomicLock;
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};

use crate::{
    arbiter::Arbiter,
    command::{Command, SessionCommand},
    subaccounts::SubAccounts,
};

pub struct BinanceOrderSending {
    observer: Arc<AtomicLock<SingleObserver<OrderConnectorNotification>>>,
    subaccounts: Arc<AtomicLock<SubAccounts>>,
    subaccount_rx: Option<UnboundedReceiver<Credentials>>,
    command_tx: UnboundedSender<SessionCommand>,
    command_rx: Option<UnboundedReceiver<SessionCommand>>,
    api_key: Option<String>,
    arbiter: Arbiter,
}

impl BinanceOrderSending {
    pub fn new() -> Self {
        let (subaccount_tx, subaccount_rx) = unbounded_channel();
        let (command_tx, command_rx) = unbounded_channel();
        Self {
            observer: Arc::new(AtomicLock::new(SingleObserver::new())),
            subaccounts: Arc::new(AtomicLock::new(SubAccounts::new(subaccount_tx))),
            subaccount_rx: Some(subaccount_rx),
            command_tx,
            command_rx: Some(command_rx),
            api_key: None,
            arbiter: Arbiter::new(),
        }
    }

    pub fn start(&mut self) -> Result<()> {
        let subaccount_rx = self
            .subaccount_rx
            .take()
            .ok_or_eyre("Subaccount receiver unavailable")?;

        let command_rx = self
            .command_rx
            .take()
            .ok_or_eyre("Session command receiver unavailable")?;

        self.arbiter.start(
            self.subaccounts.clone(),
            subaccount_rx,
            command_rx,
            self.observer.clone(),
        );

        Ok(())
    }

    pub async fn stop(&mut self) -> Result<()> {
        let (subaccount_rx, command_rx) = self
            .arbiter
            .stop()
            .await
            .map_err(|err| eyre!("Error stopping arbiter {}", err))?;

        self.subaccount_rx
            .replace(subaccount_rx)
            .is_none()
            .then_some(())
            .ok_or_eyre("Invalid state of subaccount receiver")?;

        self.command_rx
            .replace(command_rx)
            .is_none()
            .then_some(())
            .ok_or_eyre("Invalid state of command receiver")?;

        Ok(())
    }

    pub fn logon(&mut self, subaccounts: impl IntoIterator<Item = Credentials>) -> Result<()> {
        self.subaccounts.write().logon(subaccounts)
    }
}

impl OrderConnector for BinanceOrderSending {
    fn send_order(&mut self, order: &Arc<SingleOrder>) -> Result<()> {
        // TODO: we could have multiple sub-accounts, and each would be
        // identified by its API Key. However for simplicity we use single atm.
        match &self.api_key {
            Some(api_key) => self
                .command_tx
                .send(SessionCommand {
                    api_key: api_key.clone(),
                    command: Command::NewOrder(order.clone()),
                })
                .map_err(|err| eyre!("Failed to send new order: {}", err)),
            None => Err(eyre!("Session not connected")),
        }
    }
}

impl IntoObservableSingleArc<OrderConnectorNotification> for BinanceOrderSending {
    fn get_single_observer_arc(
        &mut self,
    ) -> &Arc<AtomicLock<SingleObserver<OrderConnectorNotification>>> {
        &self.observer
    }
}
