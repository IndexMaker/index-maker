use std::sync::Arc;

use async_trait::async_trait;
use eyre::{eyre, Result};
use parking_lot::RwLock as AtomicLock;
use symm_core::{
    core::{bits::Symbol, functional::MultiObserver},
    market_data::market_data_connector::{MarketDataEvent, Subscription},
};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};

use crate::subscriptions::Subscriptions;

#[async_trait]
pub trait SubscriberTask {
    fn start(
        &mut self,
        subscription_rx: UnboundedReceiver<Subscription>,
        observer: Arc<AtomicLock<MultiObserver<Arc<MarketDataEvent>>>>,
    ) -> Result<()>;

    async fn stop(&mut self) -> Result<()>;
}

pub trait SubscriberTaskFactory {
    fn create_task_for(&self, listing: &Symbol) -> Result<Box<dyn SubscriberTask + Send + Sync>>;
}

pub struct Subscriber {
    listing: Symbol,
    subscriptions: Arc<AtomicLock<Subscriptions>>,
    subscriber_task: Box<dyn SubscriberTask + Send + Sync>,
}

impl Subscriber {
    pub fn new(
        listing: Symbol,
        subscription_sender: UnboundedSender<Subscription>,
        subscriber_task: Box<dyn SubscriberTask + Send + Sync>,
    ) -> Self {
        Self {
            listing,
            subscriptions: Arc::new(AtomicLock::new(Subscriptions::new(subscription_sender))),
            subscriber_task,
        }
    }

    pub fn get_listing(&self) -> &Symbol {
        &self.listing
    }

    pub fn get_subscription_count(&self) -> usize {
        self.subscriptions.read().get_subscription_count()
    }

    pub fn subscribe(&self, subscription: &[Subscription]) -> Result<()> {
        self.subscriptions.write().subscribe(subscription)
    }

    pub async fn stop(&mut self) -> Result<()> {
        self.subscriber_task.stop().await
    }

    pub fn start(
        &mut self,
        subscription_rx: UnboundedReceiver<Subscription>,
        observer: Arc<AtomicLock<MultiObserver<Arc<MarketDataEvent>>>>,
    ) -> Result<()> {
        self.subscriber_task.start(subscription_rx, observer)
    }
}
