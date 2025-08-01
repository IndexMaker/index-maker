use std::sync::Arc;

use eyre::{eyre, Result};
use futures_util::future::join_all;
use itertools::Itertools;
use parking_lot::RwLock as AtomicLock;
use symm_core::{
    core::{bits::Symbol, functional::MultiObserver},
    market_data::market_data_connector::{MarketDataEvent, Subscription},
};
use tokio::sync::mpsc::unbounded_channel;

use crate::subscriber::{Subscriber, SubscriberTaskFactory};

pub struct Subscribers {
    subscriber_factory: Arc<dyn SubscriberTaskFactory + Send + Sync>,
    subscribers: Vec<Subscriber>,
    max_subscriber_symbols: usize,
}

impl Subscribers {
    pub fn new(
        max_subscriber_symbols: usize,
        subscriber_factory: Arc<dyn SubscriberTaskFactory + Send + Sync>,
    ) -> Self {
        Self {
            subscriber_factory,
            subscribers: Vec::new(),
            max_subscriber_symbols,
        }
    }

    pub async fn find_available_subscriber_mut(
        &mut self,
        listing: &Symbol,
    ) -> Option<&mut Subscriber> {
        for sub in &mut self.subscribers {
            if sub.get_listing().ne(listing) {
                continue;
            }
            let subscription_count = sub.get_subscription_count();
            if subscription_count < self.max_subscriber_symbols {
                return Some(sub);
            }
        }
        None
    }

    pub fn start_new_subscriber_mut(
        &mut self,
        listing: Symbol,
        observer: Arc<AtomicLock<MultiObserver<Arc<MarketDataEvent>>>>,
    ) -> Result<&mut Subscriber> {
        let (tx, rx) = unbounded_channel();
        let subscriber_task = self.subscriber_factory.create_task_for(&listing)?;
        self.subscribers
            .push(Subscriber::new(listing, tx, subscriber_task));
        let sub = self.subscribers.last_mut().unwrap();
        sub.start(rx, observer)?;
        Ok(sub)
    }

    pub async fn add_subscription(
        &mut self,
        subscrption: Subscription,
        observer: Arc<AtomicLock<MultiObserver<Arc<MarketDataEvent>>>>,
    ) -> Result<()> {
        let sub = match self
            .find_available_subscriber_mut(&subscrption.listing)
            .await
        {
            Some(sub) => {
                tracing::info!(
                    "Using existing subscriber for: {} @ {}",
                    subscrption.ticker,
                    subscrption.listing
                );
                sub
            }
            None => {
                tracing::info!(
                    "Starting new subscriber for: {} @ {}",
                    subscrption.ticker,
                    subscrption.listing
                );
                self.start_new_subscriber_mut(subscrption.listing.clone(), observer)?
            }
        };

        let subscription_clone = subscrption.clone();
        sub.subscribe(&[subscription_clone])
    }

    pub fn check_stopped(&mut self) -> Result<Vec<Subscription>> {
        let mut lost = Vec::new();

        self.subscribers.retain_mut(|s| {
            if s.has_stopped() {
                let subs = s
                    .get_subscriptions()
                    .read()
                    .get_subscriptions()
                    .iter()
                    .cloned()
                    .collect_vec();

                lost.extend(subs);
                false
            } else {
                true
            }
        });

        Ok(lost)
    }

    pub async fn stop_all(&mut self) -> Result<()> {
        let stop_futures = self
            .subscribers
            .iter_mut()
            .map(|sub| sub.stop())
            .collect_vec();

        let (_, failures): (Vec<_>, Vec<_>) =
            join_all(stop_futures).await.into_iter().partition_result();

        if !failures.is_empty() {
            Err(eyre!(
                "Subscribers join failed {}",
                failures.iter().map(|e| format!("{:?}", e)).join(";"),
            ))?;
        }

        Ok(())
    }
}
