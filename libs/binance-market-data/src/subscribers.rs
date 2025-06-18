use std::{sync::Arc, usize};

use eyre::{eyre, Result};
use futures_util::future::join_all;
use index_maker::{
    core::{bits::Symbol, functional::MultiObserver},
    market_data::market_data_connector::MarketDataEvent,
};
use itertools::Itertools;
use parking_lot::RwLock as AtomicLock;
use tokio::sync::mpsc::unbounded_channel;

use crate::subscriber::Subscriber;

pub struct Subscribers {
    subscribers: Vec<Subscriber>,
    max_subscriber_symbols: usize,
}

impl Subscribers {
    pub fn new(max_subscriber_symbols: usize) -> Self {
        Self {
            subscribers: Vec::new(),
            max_subscriber_symbols,
        }
    }

    pub async fn find_available_subscriber_mut(&mut self) -> Option<&mut Subscriber> {
        for sub in &mut self.subscribers {
            let subscription_count = sub.get_subscription_count();
            if subscription_count < self.max_subscriber_symbols {
                Some(sub);
            }
        }
        None
    }

    pub fn start_new_subscriber_mut(
        &mut self,
        observer: Arc<AtomicLock<MultiObserver<Arc<MarketDataEvent>>>>,
    ) -> &mut Subscriber {
        let (tx, rx) = unbounded_channel();
        self.subscribers.push(Subscriber::new(tx));
        let sub = self.subscribers.last_mut().unwrap();
        sub.start(rx, observer);
        sub
    }

    pub async fn add_subscription(
        &mut self,
        symbol: Symbol,
        observer: Arc<AtomicLock<MultiObserver<Arc<MarketDataEvent>>>>,
    ) -> Result<()> {
        let sub = match self.find_available_subscriber_mut().await {
            Some(sub) => sub,
            None => self.start_new_subscriber_mut(observer),
        };

        let symbol_clone = symbol.clone();
        sub.subscribe(&[symbol_clone])
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
