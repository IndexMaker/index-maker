use std::sync::Arc;

use async_trait::async_trait;
use binance_sdk::config::{ConfigurationRestApi, ConfigurationWebsocketStreams};
use binance_sdk::spot::rest_api::DepthParams;
use binance_sdk::spot::websocket_streams::BookTickerParams;
use binance_sdk::spot::SpotRestApi;
use binance_sdk::spot::{websocket_streams::DiffBookDepthParams, SpotWsStreams};

use chrono::Utc;
use eyre::{eyre, Result};
use futures_util::future::join_all;
use itertools::Itertools;
use market_data::subscriber::{SubscriberTask, SubscriberTaskFactory};
use parking_lot::RwLock as AtomicLock;
use symm_core::core::limit::Limiter;
use symm_core::market_data::market_data_connector::Subscription;
use symm_core::{
    core::{async_loop::AsyncLoop, bits::Symbol, functional::MultiObserver},
    market_data::market_data_connector::MarketDataEvent,
};
use tokio::time::sleep;
use tokio::{
    select,
    sync::mpsc::{unbounded_channel, UnboundedReceiver},
};
use tokio_util::sync::CancellationToken;

use crate::book::Books;

#[derive(Clone)]
pub struct BinanceSubscriberTaskConfig {
    pub subscription_limit_rate: usize,
    pub stale_check_period: std::time::Duration,
    pub stale_timeout: chrono::Duration,
}

pub struct BinanceSubscriberTask {
    config: BinanceSubscriberTaskConfig,
    subscriber_loop: AsyncLoop<()>,
    snapshot_loop: AsyncLoop<()>,
}

impl BinanceSubscriberTask {
    pub fn new(config: BinanceSubscriberTaskConfig) -> Self {
        Self {
            config,
            subscriber_loop: AsyncLoop::new(),
            snapshot_loop: AsyncLoop::new(),
        }
    }
}

#[async_trait]
impl SubscriberTask for BinanceSubscriberTask {
    async fn stop(&mut self) -> Result<()> {
        let stop_futures = [self.subscriber_loop.stop(), self.snapshot_loop.stop()];

        let (_, failures): (Vec<_>, Vec<_>) =
            join_all(stop_futures).await.into_iter().partition_result();

        if !failures.is_empty() {
            Err(eyre!(
                "Subscriber join failed {}",
                failures.iter().map(|e| format!("{:?}", e)).join(";"),
            ))?;
        }

        Ok(())
    }

    fn has_stopped(&self) -> bool {
        self.subscriber_loop.has_stopped()
    }

    fn start(
        &mut self,
        mut subscription_rx: UnboundedReceiver<Subscription>,
        observer: Arc<AtomicLock<MultiObserver<Arc<MarketDataEvent>>>>,
    ) -> Result<()> {
        let (snapshot_tx, mut snapshot_rx) = unbounded_channel::<Symbol>();

        let cancel_token = CancellationToken::new();

        let books = Arc::new(AtomicLock::new(Books::new_with_observer(observer)));
        let books_clone = books.clone();

        let config = self.config.clone();
        let config_clone = config.clone();

        self.subscriber_loop.start_with_cancel_token(cancel_token.clone(), async move |cancel_token| {
            let ws_streams_conf = match ConfigurationWebsocketStreams::builder().build() {
                Ok(x) => x,
                Err(err) => {
                    tracing::warn!("Failed to build websocket configuration {:?}", err);
                    return;
                }
            };

            let ws_streams_client = SpotWsStreams::production(ws_streams_conf);

            let connection = match ws_streams_client.connect().await {
                Ok(x) => x,
                Err(err) => {
                    tracing::warn!("Failed to connect websocket client {:?}", err);
                    return;
                }
            };

            // https://developers.binance.com/docs/binance-spot-api-docs/web-socket-streams#websocket-limits
            let mut rate_limiter = Limiter::new(5, chrono::TimeDelta::seconds(1));
            let mut limit_rate = async |weight| loop {
                let now = Utc::now();
                if rate_limiter.try_consume(weight, now) {
                    break;
                }
                let period = rate_limiter.waiting_period_half_limit(now);
                let millis = period.num_milliseconds() as u64;
                tracing::info!("Subscription rate limit pause for {}ms", millis);
                sleep(std::time::Duration::from_millis(millis)).await;
            };

            let mut check_period = tokio::time::interval(config.stale_check_period);

            tracing::info!("Market-Data loop started");
            loop {
                select! {
                    _ = cancel_token.cancelled() => {
                        break;
                    },
                    _ = check_period.tick() => {
                        let expiry_time = Utc::now() - config.stale_timeout;
                        match books.write().check_stale(expiry_time) {
                            Err(err) => {
                                tracing::warn!("Failed to check stale books: {:?}", err);
                            },
                            Ok((num_stale, num_subscriptions)) => {
                                if num_stale == num_subscriptions {
                                    tracing::warn!("Will reset subscriber: All subscriptions are stale");

                                    // This will terminate also snapshot tasks,
                                    // because we share that token with them.
                                    cancel_token.cancel();
                                    break;
                                }
                            }
                        };
                    }
                    Some(Subscription { ticker: symbol, listing: _ }) = subscription_rx.recv() => {
                        if let Err(err) = books.write().add_book(&symbol, snapshot_tx.clone(), false) {
                            tracing::warn!("Error adding book {:?}", err);
                            continue;
                        }

                        // We need to account for PING/PONG too
                        limit_rate(config_clone.subscription_limit_rate).await;

                        let depth_params = match DiffBookDepthParams::builder(symbol.to_string()).build() {
                            Ok(x) => x,
                            Err(err) => {
                                tracing::warn!("Failed to build diff-depth params: {}", err);
                                continue;
                            }
                        };

                        let depth_stream = match connection.diff_book_depth(depth_params).await {
                            Ok(x) => x,
                            Err(err) => {
                                tracing::warn!("Failed to subscribe to diff-depth stream: {}", err);
                                continue;
                            }
                        };

                        let ticker_params = match BookTickerParams::builder(symbol.to_string()).build() {
                            Ok(x) => x,
                            Err(err) => {
                                tracing::warn!("Failed to build book-ticker params: {}", err);
                                continue;
                            }
                        };

                        let ticker_stream = match connection.book_ticker(ticker_params).await {
                            Ok(x) => x,
                            Err(err) => {
                                tracing::warn!("Failed to subscribe to book-ticker stream: {}", err);
                                continue;
                            }
                        };

                        let symbol_clone = symbol.clone();
                        let books_clone = books.clone();

                        depth_stream.on_message(move |data| {
                            if let Err(err) = books_clone.write().apply_book_update(&symbol_clone, data) {
                                tracing::warn!("Failed to apply book update: {:?}", err);
                            }
                        });

                        let symbol_clone = symbol.clone();
                        let books_clone = books.clone();

                        ticker_stream.on_message(move |data| {
                            if let Err(err) = books_clone.write().apply_tob_update(&symbol_clone, data) {
                                tracing::warn!("Failed to apply tob update: {:?}", err);
                            }
                        });
                    },
                }
            }

            if let Err(err) = connection.disconnect().await {
                tracing::warn!("Failed to disconnect websocket client: {}", err);
            }
            tracing::info!("Market-Data loop exited");
        });

        self.snapshot_loop.start_with_cancel_token(cancel_token.clone(), async move |cancel_token| {
            tracing::info!("Snapshot loop started");
            let rest_conf = match ConfigurationRestApi::builder().build() {
                Ok(x) => x,
                Err(err) => {
                    tracing::warn!("Failed configure snapshotting client: {:?}", err);
                    return;
                }
            };

            let rest_client = SpotRestApi::production(rest_conf);
            loop {
                select! {
                    _ = cancel_token.cancelled() => {
                        break;
                    },
                    Some(symbol) = snapshot_rx.recv() => {
                        let params = match DepthParams::builder(symbol.to_string()).build() {
                            Ok(x) => x,
                            Err(err) =>  {
                                tracing::warn!("Failed to request depth snapshot for {}: {:?}", symbol, err);
                                continue;
                            }
                        };

                        let response = match rest_client.depth(params).await {
                            Ok(x) => x,
                            Err(err) => {
                                tracing::warn!("Failed to obtain depth snapshot {}: {:?}", symbol, err);
                                continue;
                            }
                        };

                        match response.data().await {
                            Ok(res) => {
                                if let Err(err) = books_clone.write().apply_snapshot(&symbol, res) {
                                    tracing::warn!("Failed to apply depth snapshot for {}: {:?}", symbol, err);
                                }
                            }
                            Err(err) => {
                                tracing::warn!("Failed to obtain depth snapshot for {}: {:?}", symbol, err);
                            }
                        }
                    }
                }
            }
            tracing::info!("Snapshot loop exited");
        });

        Ok(())
    }
}

pub struct BinanceOnlySubscriberTasks {
    config: BinanceSubscriberTaskConfig,
}

impl BinanceOnlySubscriberTasks {
    pub fn new(config: BinanceSubscriberTaskConfig) -> Self {
        Self { config }
    }
}

impl SubscriberTaskFactory for BinanceOnlySubscriberTasks {
    fn create_task_for(
        &self,
        listing: &Symbol,
    ) -> Result<Box<dyn market_data::subscriber::SubscriberTask + Send + Sync>> {
        if listing.ne("Binance") {
            Err(eyre!("Unsupported listing: {}", listing))?;
        }
        Ok(Box::new(BinanceSubscriberTask::new(self.config.clone())))
    }
}
