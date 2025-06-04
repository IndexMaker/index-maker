use std::{
    collections::{hash_map::Entry, HashMap},
    future::Future,
    marker::PhantomData,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    usize,
};

use eyre::{eyre, OptionExt, Report, Result};
use futures_util::FutureExt;
use index_maker::{
    core::{
        bits::Symbol,
        functional::{MultiObserver, PublishMany},
    },
    market_data::market_data_connector::MarketDataConnector,
};
use itertools::{Either, Itertools};
use tokio::{
    select, spawn,
    sync::{
        mpsc::{channel, unbounded_channel, Receiver, Sender, UnboundedReceiver, UnboundedSender},
        Mutex, RwLock,
    },
    task::{spawn_blocking, JoinError, JoinHandle},
};

struct Subscriptions {
    subscriptions_open: AtomicUsize,
    subscriptions_ready: AtomicUsize,
    subscription_sender: UnboundedSender<Symbol>,
}

impl Subscriptions {
    pub fn new(subscription_sender: UnboundedSender<Symbol>) -> Self {
        Self {
            subscriptions_open: AtomicUsize::new(0),
            subscriptions_ready: AtomicUsize::new(0),
            subscription_sender,
        }
    }

    fn subscribe(&self, symbols: &[index_maker::core::bits::Symbol]) -> Result<()> {
        let (successes, failures): (Vec<()>, Vec<_>) = symbols
            .iter()
            .map(|symbol| self.subscription_sender.send(symbol.clone()))
            .partition_result();

        failures.is_empty().then_some(()).ok_or_else(|| {
            eyre!(
                "Subscriptions failed {}",
                failures.iter().map(|e| format!("{:?}", e)).join(";"),
            )
        })?;

        self.subscriptions_open
            .fetch_add(successes.len(), Ordering::Relaxed);
        Ok(())
    }
}

struct Subscriber {
    subscription_rx: UnboundedReceiver<Symbol>,
}

struct Arbiter {
    arbiter_task: Option<JoinHandle<UnboundedReceiver<Symbol>>>,
    subscriber: Vec<Subscriber>,
}

impl Arbiter {
    pub fn new() -> Self {
        Self {
            arbiter_task: None,
            subscriber: Vec::new(),
        }
    }

    pub fn start(
        &mut self,
        subscriptions: Arc<Subscriptions>,
        mut subscription_rx: UnboundedReceiver<Symbol>,
        max_subscriber_symbols: usize,
    ) {
        self.arbiter_task.replace(spawn(async move {
            loop {
                let subscription = subscription_rx.recv().await;
            }
            subscription_rx
        }));
    }

    pub async fn stop(&mut self) -> Result<UnboundedReceiver<Symbol>, Either<JoinError, Report>> {
        if let Some(task) = self.arbiter_task.take() {
            task.await.map_err(|err| Either::Left(err))
        } else {
            Err(Either::Right(eyre!("Arbiter task is not running")))
        }
    }
}
struct BinanceMarketData {
    subscriptions: Arc<Subscriptions>,
    subscription_rx: Option<UnboundedReceiver<Symbol>>,
    arbiter: Arbiter,
    max_subscriber_symbols: usize,
}

impl BinanceMarketData {
    pub fn new(max_subscriber_symbols: usize) -> Self {
        let (subscription_sender, subscription_rx) = unbounded_channel();
        Self {
            subscriptions: Arc::new(Subscriptions::new(subscription_sender)),
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
            self.max_subscriber_symbols,
        );

        Ok(())
    }

    pub fn stop(&mut self) -> Result<()> {
        //let rx = spawn_blocking(async { self.arbiter.stop().await });
        Ok(())
    }
}

impl MarketDataConnector for BinanceMarketData {
    fn subscribe(&self, symbols: &[index_maker::core::bits::Symbol]) -> Result<()> {
        self.subscriptions.subscribe(symbols)
    }
}

#[tokio::main]
async fn main() {
    let mut market_data = BinanceMarketData::new(2);

    market_data.start().expect("Failed to start market data");
    market_data
        .subscribe(&["BNBUSDT".into()])
        .expect("Failed to subscribe");
}
