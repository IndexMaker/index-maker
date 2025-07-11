use std::sync::Arc;

use eyre::{eyre, OptionExt, Result};
use parking_lot::RwLock as AtomicLock;
use symm_core::{
    core::functional::{IntoObservableManyArc, MultiObserver},
    market_data::market_data_connector::{MarketDataConnector, MarketDataEvent, Subscription},
};
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver};

use crate::arbiter::Arbiter;
use crate::{subscriber::SubscriberTaskFactory, subscriptions::Subscriptions};

pub struct RealMarketData {
    observer: Arc<AtomicLock<MultiObserver<Arc<MarketDataEvent>>>>,
    subscriptions: Arc<AtomicLock<Subscriptions>>,
    subscription_rx: Option<UnboundedReceiver<Subscription>>,
    arbiter: Arbiter,
    max_subscriber_symbols: usize,
    subscriber_task_factory: Arc<dyn SubscriberTaskFactory + Send + Sync>,
}

impl RealMarketData {
    pub fn new(
        max_subscriber_symbols: usize,
        subscriber_task_factory: Arc<dyn SubscriberTaskFactory + Send + Sync>,
    ) -> Self {
        let (subscription_sender, subscription_rx) = unbounded_channel();
        Self {
            observer: Arc::new(AtomicLock::new(MultiObserver::new())),
            subscriptions: Arc::new(AtomicLock::new(Subscriptions::new(subscription_sender))),
            subscription_rx: Some(subscription_rx),
            arbiter: Arbiter::new(),
            max_subscriber_symbols,
            subscriber_task_factory,
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
            self.subscriber_task_factory.clone(),
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

impl MarketDataConnector for RealMarketData {
    fn subscribe(&self, subscriptions: &[Subscription]) -> Result<()> {
        self.subscriptions.write().subscribe(subscriptions)
    }
}

impl IntoObservableManyArc<Arc<MarketDataEvent>> for RealMarketData {
    fn get_multi_observer_arc(&self) -> &Arc<AtomicLock<MultiObserver<Arc<MarketDataEvent>>>> {
        &self.observer
    }
}
