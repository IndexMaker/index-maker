use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use binance_spot_connector_rust::{
    hyper::BinanceHttpClient,
    market::depth::Depth,
    market_stream::{book_ticker::BookTickerStream, diff_depth::DiffDepthStream},
    tokio_tungstenite::BinanceWebSocketClient,
};

use futures_util::{StreamExt, TryFutureExt};
use rand::Rng;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::{
    sync::{Notify, RwLock},
    time::sleep,
};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PriceLevel(String, String);

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DepthSnapshot {
    last_update_id: u64,
    bids: Vec<PriceLevel>,
    asks: Vec<PriceLevel>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DiffDepthUpdate {
    #[serde(rename = "U")]
    first_update_id_in_event: u64,
    #[serde(rename = "u")]
    final_update_id_in_event: u64,
    #[serde(rename = "b")]
    bids: Vec<PriceLevel>,
    #[serde(rename = "a")]
    asks: Vec<PriceLevel>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BookTickerUpdate {
    #[serde(rename = "b")]
    best_bid_price: Decimal,
    #[serde(rename = "B")]
    best_bid_quantity: Decimal,
    #[serde(rename = "a")]
    best_ask_price: Decimal,
    #[serde(rename = "A")]
    best_ask_quantity: Decimal,
    #[serde(rename = "s")]
    symbol: String,
    #[serde(rename = "u")]
    update_id: u64,
}

struct Book {
    symbol: String,
    last_update_id: Option<u64>,
    needs_snapshot: Arc<AtomicBool>,
    snapshot_notify: Arc<Notify>,
}

impl Book {
    pub fn new(
        symbol: &str,
        needs_snapshot_flag: Arc<AtomicBool>,
        snapshot_notify: Arc<Notify>,
    ) -> Self {
        Book {
            symbol: symbol.to_owned(),
            last_update_id: None,
            needs_snapshot: needs_snapshot_flag,
            snapshot_notify,
        }
    }

    pub fn apply_snapshot(&mut self, data: &str) {
        match serde_json::from_str::<DepthSnapshot>(data) {
            Ok(snapshot) => {
                println!(
                    "Snapshot received (lastUpdateId: {}). Overwriting book.",
                    snapshot.last_update_id
                );
                self.last_update_id = Some(snapshot.last_update_id);
                self.needs_snapshot.store(false, Ordering::Relaxed);
            }
            Err(e) => {
                eprintln!("Failed to parse snapshot data: {:?} - Data: {}", e, data);
                self.needs_snapshot.store(true, Ordering::Relaxed);
                self.snapshot_notify.notify_one();
            }
        }
    }

    pub fn apply_update(&mut self, data: &str) {
        if let Ok(value) = serde_json::from_str::<Value>(data) {
            if let Some(s) = value["stream"].as_str() {
                if let Some((symbol, stream_kind)) = s.split_once("@") {
                    match stream_kind {
                        "depth" => self.apply_book(symbol, value["data"].clone()),
                        "bookTicker" => self.apply_tob(symbol, value["data"].clone()),
                        _ => eprintln!("Unknown stream type {}", stream_kind),
                    }
                } else {
                    eprintln!("Unknown type of stream");
                }
            }
        } else {
            eprintln!("Failed to parse message as known type: {}", data);
        }
    }

    pub fn apply_tob(&mut self, symbol: &str, data: Value) {
        if !symbol.eq_ignore_ascii_case(&self.symbol) {
            eprintln!("Simulating invalid symbol {}", symbol);
            return;
        }
        if let Ok(_) = serde_json::from_value::<BookTickerUpdate>(data) {
            // do nothing, too many ticks to print
        } else {
            eprintln!("Cannot parse BookTickerUpdate");
        }
    }

    pub fn apply_book(&mut self, symbol: &str, data: Value) {
        if rand::rng().random_range(0.0..1.0) < 0.2 {
            println!("Simulating drop of a DiffDepthUpdate message.");
            return;
        }
        if !symbol.eq_ignore_ascii_case(&self.symbol) {
            eprintln!("Simulating invalid symbol {}", symbol);
            return;
        }
        if let Ok(diff_depth) = serde_json::from_value::<DiffDepthUpdate>(data) {
            match self.last_update_id {
                Some(last_id) => {
                    if diff_depth.first_update_id_in_event <= last_id + 1
                        && diff_depth.final_update_id_in_event >= last_id
                    {
                        println!(
                            "DiffDepthUpdate received (U: {}, u: {}). Applying update.",
                            diff_depth.first_update_id_in_event,
                            diff_depth.final_update_id_in_event
                        );
                        self.last_update_id = Some(diff_depth.final_update_id_in_event);
                    } else {
                        println!(
                            "DiffDepthUpdate out of sync (U: {}, u: {} vs last_id: {}). Requesting new snapshot.",
                            diff_depth.first_update_id_in_event,
                            diff_depth.final_update_id_in_event,
                            last_id
                        );
                        self.needs_snapshot.store(true, Ordering::Relaxed);
                        self.snapshot_notify.notify_one();
                    }
                }
                None => {
                    println!(
                        "No snapshot received yet. Cannot apply DiffDepthUpdate (U: {}, u: {}). Requesting snapshot.",
                        diff_depth.first_update_id_in_event, diff_depth.final_update_id_in_event
                    );
                    self.needs_snapshot.store(true, Ordering::Relaxed);
                    self.snapshot_notify.notify_one();
                }
            }
        } else {
            eprintln!("Cannot parse DiffDepthUpdate");
        }
    }
}

#[tokio::main]
async fn main() {
    let symbol: &'static str = "BNBUSDT";

    let needs_snapshot_flag = Arc::new(AtomicBool::new(true));
    let needs_snapshot_flag_clone_for_book = needs_snapshot_flag.clone();
    let needs_snapshot_flag_clone_for_snapshot_task = needs_snapshot_flag.clone();

    let snapshot_notify = Arc::new(Notify::new());
    let snapshot_notify_clone_for_book = snapshot_notify.clone();
    let snapshot_notify_clone_for_snapshot_task = snapshot_notify.clone();

    let book = Arc::new(RwLock::new(Book::new(
        symbol,
        needs_snapshot_flag_clone_for_book,
        snapshot_notify_clone_for_book,
    )));
    let book_clone_for_update_task = book.clone();
    let book_clone_for_snapshot_task = book.clone();

    let stop_update_task_flag = Arc::new(AtomicBool::new(false));
    let stop_update_task_flag_clone = stop_update_task_flag.clone();

    let update_task = tokio::spawn(async move {
        let (mut conn, _) = BinanceWebSocketClient::connect_async_default()
            .await
            .expect("failed to connect to Binance");

        conn.subscribe(vec![
            &BookTickerStream::from_symbol(symbol).into(),
            &DiffDepthStream::from_1000ms(symbol).into(),
        ])
        .await;

        println!("Subscribed to WebSocket streams.");

        while let Some(message) = conn.as_mut().next().await {
            match message {
                Ok(message) => {
                    let binary_data = message.into_data();
                    let data = std::str::from_utf8(&binary_data).expect("Failed to parse message");
                    book_clone_for_update_task.write().await.apply_update(data);
                }
                Err(e) => {
                    eprintln!("WebSocket error: {:?}", e);
                    break;
                }
            }
            if stop_update_task_flag_clone.load(Ordering::Relaxed) {
                println!("Stopping update task due to stop signal.");
                break;
            }
        }

        conn.close().await.expect("Failed to disconnect");
        println!("WebSocket connection closed.");
    });

    let snapshot_task = tokio::spawn(async move {
        let http_client = BinanceHttpClient::default();

        loop {
            snapshot_notify_clone_for_snapshot_task.notified().await;

            if !needs_snapshot_flag_clone_for_snapshot_task.load(Ordering::Relaxed) {
                println!("Stopping snapshot task due to unified stop signal.");
                break;
            }

            println!("Snapshot needed. Fetching new snapshot...");
            match http_client
                .send(Depth::new(symbol))
                .and_then(|res| res.into_body_str())
                .await
            {
                Ok(snapshot_data) => {
                    book_clone_for_snapshot_task
                        .write()
                        .await
                        .apply_snapshot(&snapshot_data);
                    println!("Snapshot applied successfully.");
                }
                Err(e) => {
                    eprintln!("Failed to obtain or parse depth snapshot: {:?}", e);
                    snapshot_notify_clone_for_snapshot_task.notify_one();
                    sleep(Duration::from_secs(1)).await;
                }
            }
        }
    });

    println!("Main thread sleeping for 15 seconds...");
    sleep(Duration::from_secs(15)).await;

    println!("Main thread signaling tasks to stop...");
    stop_update_task_flag.store(true, Ordering::Relaxed);

    needs_snapshot_flag.store(false, Ordering::Relaxed);
    snapshot_notify.notify_one();

    let _ = update_task.await;
    let _ = snapshot_task.await;

    println!("Application finished.");
}

