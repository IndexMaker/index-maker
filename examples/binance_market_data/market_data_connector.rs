use std::{collections::HashSet, future::Future, sync::Arc, time::Duration, usize};

use eyre::{eyre, OptionExt, Report, Result};
use futures_util::FutureExt;
use index_maker::{core::bits::Symbol, market_data::market_data_connector::MarketDataConnector};
use itertools::{Either, Itertools};
use parking_lot::RwLock as AtomicLock;
use tokio::{
    select, spawn,
    sync::{
        mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender},
        RwLock,
    },
    task::{JoinError, JoinHandle},
    time::sleep,
};
use tokio_util::sync::CancellationToken;

struct AsyncTask<T> {
    join_handle: JoinHandle<T>,
    cancel_token: CancellationToken,
}

impl<T> AsyncTask<T> {
    pub fn new(join_handle: JoinHandle<T>, cancel_token: CancellationToken) -> Self {
        Self {
            join_handle,
            cancel_token,
        }
    }

    pub async fn stop(self) -> Result<T, JoinError> {
        self.cancel_token.cancel();
        self.join_handle.await
    }
}

struct AsyncLoop<T> {
    async_task: Option<AsyncTask<T>>,
}

impl<T> AsyncLoop<T>
where
    T: Send + 'static,
{
    pub fn new() -> Self {
        Self { async_task: None }
    }

    pub fn start<Fut>(&mut self, f: impl FnOnce(CancellationToken) -> Fut)
    where
        Fut: Future<Output = T> + Send + 'static,
    {
        let cancel_token = CancellationToken::new();
        let cancel_token_cloned = cancel_token.clone();

        self.async_task
            .replace(AsyncTask::new(spawn(f(cancel_token_cloned)), cancel_token));
    }

    pub async fn stop(&mut self) -> Result<T, Either<JoinError, Report>> {
        if let Some(task) = self.async_task.take() {
            task.stop().await.map_err(|err| Either::Left(err))
        } else {
            Err(Either::Right(eyre!("AsyncLoop is not running")))
        }
    }
}

struct AsyncLoop2<T> {
    async_task: Option<AsyncTask<UnboundedReceiver<T>>>,
}

impl<T> AsyncLoop2<T>
where
    T: Send + 'static,
{
    pub fn new() -> Self {
        Self { async_task: None }
    }

    pub fn start<F, Fut>(&mut self, mut receiver: UnboundedReceiver<T>, mut f: F)
    where
        F: FnMut(T) -> Fut + Send + 'static,
        Fut: Future<Output = ()> + Send + 'static,
    {
        let cancel_token = CancellationToken::new();
        let cancel_token_cloned = cancel_token.clone();

        self.async_task.replace(AsyncTask::new(
            spawn(async move {
                loop {
                    select! {
                        _ = cancel_token_cloned.cancelled() => {
                            break;
                        },
                        Some(val) = receiver.recv() => {
                            f(val).await;
                        }
                    }
                }
                receiver
            }),
            cancel_token,
        ));
    }

    pub async fn stop(&mut self) -> Result<UnboundedReceiver<T>, Either<JoinError, Report>> {
        if let Some(task) = self.async_task.take() {
            task.stop().await.map_err(|err| Either::Left(err))
        } else {
            Err(Either::Right(eyre!("AsyncLoop is not running")))
        }
    }
}

struct Subscriptions {
    subscription_sender: UnboundedSender<Symbol>,
    subscriptions: HashSet<Symbol>,
    subscriptions_taken: HashSet<Symbol>,
}

impl Subscriptions {
    pub fn new(subscription_sender: UnboundedSender<Symbol>) -> Self {
        Self {
            subscription_sender,
            subscriptions: HashSet::new(),
            subscriptions_taken: HashSet::new(),
        }
    }

    pub fn get_subscriptions(&self) -> &HashSet<Symbol> {
        &self.subscriptions
    }

    pub fn get_subscription_count(&self) -> usize {
        self.subscriptions.len()
    }

    pub fn get_subscriptions_taken(&self) -> usize {
        self.subscriptions_taken.len()
    }

    pub fn add_subscription_taken(&mut self, symbol: Symbol) -> Result<()> {
        self.subscriptions
            .contains(&symbol)
            .then_some(())
            .ok_or_eyre("Subscription not found")?;
        self.subscriptions_taken
            .insert(symbol)
            .then_some(())
            .ok_or_eyre("Subscription already taken")?;
        Ok(())
    }

    pub fn subscribe(&mut self, symbols: &[index_maker::core::bits::Symbol]) -> Result<()> {
        let (successes, failures): (Vec<_>, Vec<_>) = symbols
            .iter()
            .map(|symbol| {
                self.subscription_sender
                    .send(symbol.clone())
                    .map(|_| symbol.clone())
            })
            .partition_result();

        if !failures.is_empty() {
            Err(eyre!(
                "Subscriptions failed {}",
                failures.iter().map(|e| format!("{:?}", e)).join(";"),
            ))?;
        }

        self.subscriptions.extend(successes.into_iter());
        Ok(())
    }
}

struct Subscriber {
    subscriptions: Arc<AtomicLock<Subscriptions>>,
    subscriber_task: Option<AsyncTask<UnboundedReceiver<Symbol>>>,
}

impl Subscriber {
    pub fn new(subscription_sender: UnboundedSender<Symbol>) -> Self {
        Self {
            subscriptions: Arc::new(AtomicLock::new(Subscriptions::new(subscription_sender))),
            subscriber_task: None,
        }
    }
    pub fn start(&mut self, mut subscription_rx: UnboundedReceiver<Symbol>) {
        let cancel_token = CancellationToken::new();
        let cancel_token_cloned = cancel_token.clone();

        let subscriber_task = spawn(async move {
            loop {
                select! {
                    _ = cancel_token_cloned.cancelled() => {
                        break;
                    },
                    Some(symbol) = subscription_rx.recv() => {
                        ;
                    }
                }
            }
            subscription_rx
        });

        self.subscriber_task
            .replace(AsyncTask::new(subscriber_task, cancel_token));
    }

    pub async fn stop(&mut self) -> Result<UnboundedReceiver<Symbol>, Either<JoinError, Report>> {
        if let Some(task) = self.subscriber_task.take() {
            task.stop().await.map_err(|err| Either::Left(err))
        } else {
            Err(Either::Right(eyre!("Subscriber task is not running")))
        }
    }
}

struct Subscribers {
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
            let subscription_count = sub.subscriptions.read().get_subscription_count();
            if subscription_count < self.max_subscriber_symbols {
                Some(sub);
            }
        }
        None
    }

    pub fn start_new_subscriber_mut(&mut self) -> &mut Subscriber {
        let (tx, rx) = unbounded_channel();
        self.subscribers.push(Subscriber::new(tx));
        let sub = self.subscribers.last_mut().unwrap();
        sub.start(rx);
        sub
    }

    pub async fn add_subscription(&mut self, symbol: Symbol) -> Result<()> {
        let sub = match self.find_available_subscriber_mut().await {
            Some(sub) => sub,
            None => self.start_new_subscriber_mut(),
        };

        let symbol_clone = symbol.clone();
        sub.subscriptions.write().subscribe(&[symbol_clone])
    }
}

struct Arbiter2 {
    arbiter_loop: AsyncLoop2<Symbol>,
}

impl Arbiter2 {
    pub fn new() -> Self {
        Self {
            arbiter_loop: AsyncLoop2::new(),
        }
    }

    pub async fn stop(&mut self) -> Result<UnboundedReceiver<Symbol>, Either<JoinError, Report>> {
        self.arbiter_loop.stop().await
    }

    pub fn start(
        &mut self,
        subscriptions: Arc<AtomicLock<Subscriptions>>,
        subscription_rx: UnboundedReceiver<Symbol>,
        max_subscriber_symbols: usize,
    ) {
        let subscribers = Arc::new(RwLock::new(Subscribers::new(max_subscriber_symbols)));
        self.arbiter_loop.start(subscription_rx, move |symbol| {
            let subscriptions = subscriptions.clone();
            let subscribers = subscribers.clone();
            async move {
                match subscribers
                    .write()
                    .await
                    .add_subscription(symbol.clone())
                    .await
                {
                    Ok(_) => {
                        let mut subs = subscriptions.write();
                        if let Err(err) = subs.add_subscription_taken(symbol) {
                            eprintln!("Error storing taken subscription {}", err);
                        }
                    }
                    Err(err) => {
                        eprintln!("Error while subscribing {}", err);
                    }
                }
            }
        });
    }
}

struct Arbiter {
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
        max_subscriber_symbols: usize,
    ) {
        let mut subscribers = Subscribers::new(max_subscriber_symbols);
        self.arbiter_loop.start(async move |cancel_token| {
            loop {
                select! {
                    _ = cancel_token.cancelled() => {
                        break
                    },
                    Some(symbol) = subscription_rx.recv() => {
                        match subscribers.add_subscription(symbol.clone()).await {
                            Ok(_) => {
                                let mut subs = subscriptions.write();
                                if let Err(err) = subs.add_subscription_taken(symbol) {
                                    eprintln!("Error storing taken subscription {}", err);
                                }
                            }
                            Err(err) => {
                                eprintln!("Error while subscribing {}", err);
                            }
                        }
                    }
                }
            }
            subscription_rx
        });
    }
}

struct BinanceMarketData {
    subscriptions: Arc<AtomicLock<Subscriptions>>,
    subscription_rx: Option<UnboundedReceiver<Symbol>>,
    arbiter: Arbiter,
    max_subscriber_symbols: usize,
}

impl BinanceMarketData {
    pub fn new(max_subscriber_symbols: usize) -> Self {
        let (subscription_sender, subscription_rx) = unbounded_channel();
        Self {
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
    fn subscribe(&self, symbols: &[index_maker::core::bits::Symbol]) -> Result<()> {
        self.subscriptions.write().subscribe(symbols)
    }
}

#[tokio::main]
async fn main() {
    let mut market_data = BinanceMarketData::new(2);

    market_data.start().expect("Failed to start market data");
    market_data
        .subscribe(&["BNBUSDT".into()])
        .expect("Failed to subscribe");

    sleep(Duration::from_secs(15)).await;

    market_data
        .stop()
        .await
        .expect("Failed to stop market data");
}
