use std::{sync::Arc, time::Duration};

use binance_spot_connector_rust::{
    hyper::BinanceHttpClient,
    market::depth::Depth,
    market_stream::{book_ticker::BookTickerStream, diff_depth::DiffDepthStream},
    tokio_tungstenite::BinanceWebSocketClient,
};
use eyre::{eyre, Result};
use futures_util::{future::join_all, StreamExt};
use index_maker::core::bits::Symbol;
use itertools::Itertools;
use parking_lot::RwLock as AtomicLock;
use rand::Rng;
use serde_json::Value;
use tokio::{
    select,
    sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender}, time::sleep,
};

use crate::{async_loop::AsyncLoop, book::Books, subscriptions::Subscriptions};

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

    pub fn start(&mut self, mut subscription_rx: UnboundedReceiver<Symbol>) {
        let (snapshot_tx, mut snapshot_rx) = unbounded_channel::<Symbol>();

        let books = Arc::new(AtomicLock::new(Books::new()));
        let books_clone = books.clone();

        self.subscriber_loop.start(async move |cancel_token| {
            let (mut conn, _) = BinanceWebSocketClient::connect_async_default()
                .await
                .expect("failed to connect to Binance");
            loop {
                select! {
                    _ = cancel_token.cancelled() => {
                        break;
                    },
                    Some(symbol) = subscription_rx.recv() => {
                        if let Err(err) = books.write().add_book(&symbol, snapshot_tx.clone()) {
                            eprintln!("Error adding book {:?}", err);
                        }
                        conn.subscribe(vec![
                            &BookTickerStream::from_symbol(&symbol).into(),
                            &DiffDepthStream::from_1000ms(&symbol).into(),
                        ])
                        .await;
                    },
                    Some(event) = conn.as_mut().next() => {
                        if rand::rng().random_range(0.0..1.0) < 0.02 {
                            continue;
                        }
                        match event {
                            Ok(message) => {
                                let binary_data = message.into_data();
                                match std::str::from_utf8(&binary_data) {
                                    Ok(data) => {
                                        if data == "{\"result\":null,\"id\":0}" {
                                            println!("Subscription response");
                                        }
                                        else if let Ok(value) = serde_json::from_str::<Value>(data) {
                                            if let Some(_) = value["stream"].as_str() {
                                                if let Err(err) = books.write().apply_update(value) {
                                                    eprintln!("Failed to apply book update: {:?}", err);
                                                }
                                            }
                                            else {
                                                eprintln!("Failed to ingest message: {:?}", data);
                                            }
                                        } else {
                                            eprintln!("Failed to parse message: {:?}", data);
                                        }
                                    },
                                    Err(err) => {
                                        eprintln!("Failed to parse book update as UTF8: {:?}", err);
                                    }
                                }
                            }
                            Err(err) => {
                                eprintln!("Failed to receive book update: {:?}", err);

                            }
                        }
                    }
                }
            }
        });

        self.snapshot_loop.start(async move |cancel_token| {
            let http_client = BinanceHttpClient::default();
            loop {
                select! {
                    _ = cancel_token.cancelled() => {
                        break;
                    },
                    Some(symbol) = snapshot_rx.recv() => {
                        match http_client.send(Depth::new(&symbol)).await {
                            Ok(res) => {
                                match res.into_body_str().await {
                                    Ok(snapshot_data) => {
                                        println!("Sleeping to simulate delay and delta accumulation");
                                        sleep(Duration::from_secs(1)).await;
                                        if let Err(err) = books_clone.write().apply_snapshot(&symbol, &snapshot_data) {
                                            eprintln!("Failed to apply depth snapshot: {:?}", err);
                                        }
                                    }
                                    Err(err) => {
                                        eprintln!("Failed to obtain or parse depth snapshot: {:?}", err);
                                    }
                                }
                            }
                            Err(err) => {
                                eprintln!("Failed to obtain depth snapshot: {:?}", err);
                            }
                        }
                    }
                }
            }
        });
    }
}
