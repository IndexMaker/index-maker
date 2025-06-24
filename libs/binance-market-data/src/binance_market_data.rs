use std::{sync::Arc, usize};

use eyre::{eyre, OptionExt, Result};
use parking_lot::RwLock as AtomicLock;
use symm_core::{
    core::{
        bits::Symbol,
        functional::{IntoObservableManyArc, MultiObserver},
    },
    market_data::market_data_connector::{MarketDataConnector, MarketDataEvent},
};
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver};

use crate::arbiter::Arbiter;
use crate::subscriptions::Subscriptions;
pub struct BinanceMarketData {
    observer: Arc<AtomicLock<MultiObserver<Arc<MarketDataEvent>>>>,
    subscriptions: Arc<AtomicLock<Subscriptions>>,
    subscription_rx: Option<UnboundedReceiver<Symbol>>,
    arbiter: Arbiter,
    max_subscriber_symbols: usize,
}

impl BinanceMarketData {
    pub fn new(max_subscriber_symbols: usize) -> Self {
        let (subscription_sender, subscription_rx) = unbounded_channel();
        Self {
            observer: Arc::new(AtomicLock::new(MultiObserver::new())),
            subscriptions: Arc::new(AtomicLock::new(Subscriptions::new(subscription_sender))),
            subscription_rx: Some(subscription_rx),
            arbiter: Arbiter::new(),
            max_subscriber_symbols,
        }
    }

    pub fn start(&mut self) -> Result<()> {
        let subscription_rx = self
            .subscription_rx
            .take()
            .ok_or_eyre("Subscription receiver unavailable")?;

        self.arbiter.start(
            self.subscriptions.clone(),
            subscription_rx,
            self.observer.clone(),
            self.max_subscriber_symbols,
        );

        Ok(())
    }

    pub async fn stop(&mut self) -> Result<()> {
        let rx = self
            .arbiter
            .stop()
            .await
            .map_err(|err| eyre!("Error stopping arbiter {}", err))?;

        self.subscription_rx
            .replace(rx)
            .is_none()
            .then_some(())
            .ok_or_eyre("Invalid state")?;

        Ok(())
    }
}

impl MarketDataConnector for BinanceMarketData {
    fn subscribe(&self, symbols: &[Symbol]) -> Result<()> {
        self.subscriptions.write().subscribe(symbols)
    }
}

impl IntoObservableManyArc<Arc<MarketDataEvent>> for BinanceMarketData {
    fn get_multi_observer_arc(&mut self) -> &Arc<AtomicLock<MultiObserver<Arc<MarketDataEvent>>>> {
        &self.observer
    }
}
