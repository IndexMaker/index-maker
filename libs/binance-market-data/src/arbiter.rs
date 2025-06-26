use std::sync::Arc;

use eyre::{Report, Result};
use itertools::Either;
use parking_lot::RwLock as AtomicLock;
use symm_core::core::bits::Symbol;
use symm_core::core::functional::MultiObserver;
use symm_core::market_data::market_data_connector::MarketDataEvent;
use tokio::{select, sync::mpsc::UnboundedReceiver, task::JoinError};

use symm_core::core::async_loop::AsyncLoop;

use crate::subscribers::Subscribers;
use crate::subscriptions::Subscriptions;

pub struct Arbiter {
    arbiter_loop: AsyncLoop<UnboundedReceiver<Symbol>>,
}

impl Arbiter {
    pub fn new() -> Self {
        Self {
            arbiter_loop: AsyncLoop::new(),
        }
    }

    pub async fn stop(&mut self) -> Result<UnboundedReceiver<Symbol>, Either<JoinError, Report>> {
        self.arbiter_loop.stop().await
    }

    pub fn start(
        &mut self,
        subscriptions: Arc<AtomicLock<Subscriptions>>,
        mut subscription_rx: UnboundedReceiver<Symbol>,
        observer: Arc<AtomicLock<MultiObserver<Arc<MarketDataEvent>>>>,
        max_subscriber_symbols: usize,
    ) {
        let mut subscribers = Subscribers::new(max_subscriber_symbols);
        self.arbiter_loop.start(async move |cancel_token| {
            tracing::info!("Loop started");
            loop {
                select! {
                    _ = cancel_token.cancelled() => {
                        break
                    },
                    Some(symbol) = subscription_rx.recv() => {
                        match subscribers.add_subscription(symbol.clone(), observer.clone()).await {
                            Ok(_) => {
                                let mut subs = subscriptions.write();
                                if let Err(err) = subs.add_subscription_taken(symbol) {
                                    tracing::warn!("Error storing taken subscription {:?}", err);
                                }
                            }
                            Err(err) => {
                                tracing::warn!("Error while subscribing {:?}", err);
                            }
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
