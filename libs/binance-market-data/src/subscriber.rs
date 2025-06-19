use std::sync::Arc;

use binance_sdk::config::{ConfigurationRestApi, ConfigurationWebsocketStreams};
use binance_sdk::spot::rest_api::DepthParams;
use binance_sdk::spot::websocket_streams::BookTickerParams;
use binance_sdk::spot::SpotRestApi;
use binance_sdk::spot::{websocket_streams::DiffBookDepthParams, SpotWsStreams};

use eyre::{eyre, Result};
use futures_util::future::join_all;
use index_maker::{
    core::{bits::Symbol, functional::MultiObserver},
    market_data::market_data_connector::MarketDataEvent,
};
use itertools::Itertools;
use parking_lot::RwLock as AtomicLock;
use tokio::{
    select,
    sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender},
};

use crate::{book::Books, subscriptions::Subscriptions};
use async_core::async_loop::AsyncLoop;

pub struct Subscriber {
    subscriptions: Arc<AtomicLock<Subscriptions>>,
    subscriber_loop: AsyncLoop<()>,
    snapshot_loop: AsyncLoop<()>,
}

impl Subscriber {
    pub fn new(subscription_sender: UnboundedSender<Symbol>) -> Self {
        Self {
            subscriptions: Arc::new(AtomicLock::new(Subscriptions::new(subscription_sender))),
            subscriber_loop: AsyncLoop::new(),
            snapshot_loop: AsyncLoop::new(),
        }
    }

    pub fn get_subscription_count(&self) -> usize {
        self.subscriptions.read().get_subscription_count()
    }

    pub fn subscribe(&self, symbols: &[Symbol]) -> Result<()> {
        self.subscriptions.write().subscribe(symbols)
    }

    pub async fn stop(&mut self) -> Result<()> {
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

    pub fn start(
        &mut self,
        mut subscription_rx: UnboundedReceiver<Symbol>,
        observer: Arc<AtomicLock<MultiObserver<Arc<MarketDataEvent>>>>,
    ) {
        let (snapshot_tx, mut snapshot_rx) = unbounded_channel::<Symbol>();

        let books = Arc::new(AtomicLock::new(Books::new_with_observer(observer)));
        let books_clone = books.clone();

        self.subscriber_loop.start(async move |cancel_token| {
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

            loop {
                tracing::info!("Loop started");
                select! {
                    _ = cancel_token.cancelled() => {
                        break;
                    },
                    Some(symbol) = subscription_rx.recv() => {
                        if let Err(err) = books.write().add_book(&symbol, snapshot_tx.clone()) {
                            tracing::warn!("Error adding book {:?}", err);
                            continue;
                        }

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
        });

        self.snapshot_loop.start(async move |cancel_token| {
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
                                tracing::warn!("Failed to request depth snapshot: {:?}", err);
                                continue;
                            }
                        };

                        let response = match rest_client.depth(params).await {
                            Ok(x) => x,
                            Err(err) => {
                                tracing::warn!("Failed to obtain depth snapshot: {:?}", err);
                                continue;
                            }
                        };

                        match response.data().await {
                            Ok(res) => {
                                if let Err(err) = books_clone.write().apply_snapshot(&symbol, res) {
                                    tracing::warn!("Failed to apply depth snapshot: {:?}", err);
                                }
                            }
                            Err(err) => {
                                tracing::warn!("Failed to obtain depth snapshot: {:?}", err);
                            }
                        }
                    }
                }
            }
        });
    }
}
