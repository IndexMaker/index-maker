use std::sync::Arc;

use eyre::{Report, Result};
use itertools::Either;
use parking_lot::RwLock as AtomicLock;
use symm_core::core::functional::MultiObserver;
use symm_core::market_data::market_data_connector::{MarketDataEvent, Subscription};
use tokio::{select, sync::mpsc::UnboundedReceiver, task::JoinError};

use symm_core::core::async_loop::AsyncLoop;

use crate::subscriber::SubscriberTaskFactory;
use crate::subscribers::Subscribers;
use crate::subscriptions::Subscriptions;

pub struct Arbiter {
    arbiter_loop: AsyncLoop<UnboundedReceiver<Subscription>>,
}

impl Arbiter {
    pub fn new() -> Self {
        Self {
            arbiter_loop: AsyncLoop::new(),
        }
    }

    pub async fn stop(
        &mut self,
    ) -> Result<UnboundedReceiver<Subscription>, Either<JoinError, Report>> {
        self.arbiter_loop.stop().await
    }

    pub fn start(
        &mut self,
        subscriptions: Arc<AtomicLock<Subscriptions>>,
        mut subscription_rx: UnboundedReceiver<Subscription>,
        observer: Arc<AtomicLock<MultiObserver<Arc<MarketDataEvent>>>>,
        max_subscriber_symbols: usize,
        subscription_check_period: std::time::Duration,
        subscriber_task_factory: Arc<dyn SubscriberTaskFactory + Send + Sync>,
    ) {
        tracing::info!(
            "Starting arbiter with max {} symbols per subscription",
            max_subscriber_symbols
        );
        let mut check_period = tokio::time::interval(subscription_check_period);
        let mut subscribers = Subscribers::new(max_subscriber_symbols, subscriber_task_factory);
        self.arbiter_loop.start(async move |cancel_token| {
            tracing::info!("Loop started");
            loop {
                select! {
                    _ = cancel_token.cancelled() => {
                        break
                    },
                    _ = check_period.tick() => {
                        match subscribers.check_stopped() {
                            Ok(lost_subscriptions) => {
                                tracing::info!("Recovering lost subscriptions: {:?}", lost_subscriptions);
                                if let Err(err) = subscriptions.write().subscribe(&lost_subscriptions) {
                                    tracing::warn!("Failed to resubscribe: {:?}", err)
                                }
                            }
                            Err(err) => {
                                tracing::warn!("Error while watching subscriptions {:?}", err);
                            }
                        }
                    }
                    Some(subscription) = subscription_rx.recv() => {
                        if let Err(err) = subscribers.add_subscription(subscription.clone(), observer.clone()).await {
                            tracing::warn!("Error while subscribing {:?}", err);
                        }
                    }
                }
            }
            if let Err(err) = subscribers.stop_all().await {
                tracing::warn!("Error stopping subscribers {:?}", err);
            }
            tracing::info!("Loop exited");
            subscription_rx
        });
    }
}
