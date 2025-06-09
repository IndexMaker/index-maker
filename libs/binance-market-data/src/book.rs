use std::{
    collections::{hash_map::Entry, HashMap, VecDeque},
    sync::Arc,
};

use eyre::{eyre, Result};
use index_maker::{
    core::{
        bits::{Amount, PricePointEntry, Symbol},
        functional::{MultiObserver, PublishMany},
    },
    market_data::market_data_connector::MarketDataEvent,
};
use itertools::Itertools;
use parking_lot::RwLock as AtomicLock;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::mpsc::UnboundedSender;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PriceLevel(Amount, Amount);

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
    best_bid_price: Amount,
    #[serde(rename = "B")]
    best_bid_quantity: Amount,
    #[serde(rename = "a")]
    best_ask_price: Amount,
    #[serde(rename = "A")]
    best_ask_quantity: Amount,
    #[serde(rename = "s")]
    symbol: Symbol,
    #[serde(rename = "u")]
    update_id: u64,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AggregateTrade {
  #[serde(rename="e")]
  event_type: String,
  #[serde(rename="E")]
  event_time: u64,
  #[serde(rename="s")]
  symbol: Symbol,
  #[serde(rename="a")]
  aggregate_trade_id: u64,
  #[serde(rename="p")]
  price: Amount,
  #[serde(rename="q")]
  quantity: Amount,
  #[serde(rename="f")]
  first_trade_id: u64,
  #[serde(rename="l")]
  last_trade_id: u64,
  #[serde(rename="T")]
  trade_time: u64,
  #[serde(rename="m")]
  is_buyer_market_maker: bool,
}

struct Book {
    observer: Arc<AtomicLock<MultiObserver<Arc<MarketDataEvent>>>>,
    symbol: Symbol,
    snapshot_tx: UnboundedSender<Symbol>,
    pending_updates: VecDeque<DiffDepthUpdate>,
    last_update_id: Option<u64>,
    snapshot_requested: bool,
}

impl Book {
    fn new_with_observer(
        symbol: Symbol,
        snapshot_tx: UnboundedSender<Symbol>,
        observer: Arc<AtomicLock<MultiObserver<Arc<MarketDataEvent>>>>,
    ) -> Self {
        Book {
            observer,
            snapshot_tx,
            symbol,
            pending_updates: VecDeque::new(),
            last_update_id: None,
            snapshot_requested: false,
        }
    }

    fn request_snapshot(&mut self) -> Result<()> {
        println!("(binance-book) Requesting snapshot for {}", self.symbol);
        self.pending_updates.clear();
        self.snapshot_requested = true;
        self.snapshot_tx
            .send(self.symbol.clone())
            .map_err(|err| eyre!("Failed to request snapshot for {}: {:?}", self.symbol, err))
    }

    fn apply_snapshot(&mut self, snapshot: DepthSnapshot) -> Result<()> {
        if !self.snapshot_requested {
            Err(eyre!(
                "Received snapshot that was not requested for {}",
                self.symbol
            ))?;
        }
        println!(
            "(binance-book) Snapshot received for {} and will overwrite book (lastUpdateId: {})",
            self.symbol, snapshot.last_update_id
        );
        self.last_update_id = Some(snapshot.last_update_id);
        self.snapshot_requested = false;
        self.observer
            .read()
            .publish_many(&Arc::new(MarketDataEvent::OrderBookSnapshot {
                symbol: self.symbol.clone(),
                sequence_number: snapshot.last_update_id,
                bid_updates: snapshot
                    .bids
                    .into_iter()
                    .map(|PriceLevel(price, quantity)| PricePointEntry { price, quantity })
                    .collect_vec(),
                ask_updates: snapshot
                    .asks
                    .into_iter()
                    .map(|PriceLevel(price, quantity)| PricePointEntry { price, quantity })
                    .collect_vec(),
            }));
        let pending_updates = self.pending_updates.drain(..).collect_vec();
        for diff_depth in pending_updates {
            self.apply_update(diff_depth)?;
        }
        Ok(())
    }

    fn apply_update(&mut self, diff_depth: DiffDepthUpdate) -> Result<()> {
        match self.last_update_id {
            Some(last_id) => {
                if diff_depth.first_update_id_in_event > last_id + 1 {
                    println!(
                            "(binance-book) DiffDepthUpdate for {} is ahead and snapshot is needed (U: {}, u: {} vs last_id: {})",
                            self.symbol,
                            diff_depth.first_update_id_in_event,
                            diff_depth.final_update_id_in_event,
                            last_id,
                        );
                    if !self.snapshot_requested {
                        self.request_snapshot()?;
                    }
                    self.pending_updates.push_back(diff_depth);
                } else if diff_depth.final_update_id_in_event < last_id {
                    println!(
                            "(binance-book) DiffDepthUpdate for {} is old and will be ignored (U: {}, u: {} vs last_id: {})",
                            self.symbol,
                            diff_depth.first_update_id_in_event,
                            diff_depth.final_update_id_in_event,
                            last_id,
                        );
                } else {
                    println!(
                        "(binance-book) DiffDepthUpdate for {} is new and will be applied (U: {}, u: {})",
                        self.symbol,
                        diff_depth.first_update_id_in_event,
                        diff_depth.final_update_id_in_event,
                    );
                    self.last_update_id = Some(diff_depth.final_update_id_in_event);
                    self.observer
                        .read()
                        .publish_many(&Arc::new(MarketDataEvent::OrderBookDelta {
                            symbol: self.symbol.clone(),
                            sequence_number: diff_depth.final_update_id_in_event,
                            bid_updates: diff_depth
                                .bids
                                .into_iter()
                                .map(|PriceLevel(price, quantity)| PricePointEntry {
                                    price,
                                    quantity,
                                })
                                .collect_vec(),
                            ask_updates: diff_depth
                                .asks
                                .into_iter()
                                .map(|PriceLevel(price, quantity)| PricePointEntry {
                                    price,
                                    quantity,
                                })
                                .collect_vec(),
                        }));
                }
            }
            None => {
                println!(
                    "(binance-book) DiffDepthUpdate for {} empty book and snapshot is needed (U: {}, u: {} vs last_id: None)",
                    self.symbol,
                    diff_depth.first_update_id_in_event,
                    diff_depth.final_update_id_in_event,
                );
                if !self.snapshot_requested {
                    self.request_snapshot()?;
                }
                self.pending_updates.push_back(diff_depth);
            }
        }
        Ok(())
    }
}

pub struct Books {
    observer: Arc<AtomicLock<MultiObserver<Arc<MarketDataEvent>>>>,
    books: HashMap<Symbol, Book>,
}

impl Books {
    pub fn new_with_observer(
        observer: Arc<AtomicLock<MultiObserver<Arc<MarketDataEvent>>>>,
    ) -> Self {
        Self {
            observer,
            books: HashMap::new(),
        }
    }

    pub fn add_book(
        &mut self,
        symbol: &Symbol,
        snapshot_tx: UnboundedSender<Symbol>,
    ) -> Result<()> {
        match self.books.entry(symbol.clone()) {
            Entry::Vacant(entry) => {
                entry.insert(Book::new_with_observer(
                    symbol.clone(),
                    snapshot_tx.clone(),
                    self.observer.clone(),
                ));
                Ok(())
            }
            Entry::Occupied(_) => Err(eyre!("Book already exists {}", symbol)),
        }
    }

    pub fn apply_snapshot(&mut self, symbol: &Symbol, data: &str) -> Result<()> {
        match serde_json::from_str::<DepthSnapshot>(data) {
            Ok(snapshot) => match self.books.entry(symbol.clone()) {
                Entry::Occupied(mut entry) => entry.get_mut().apply_snapshot(snapshot),
                Entry::Vacant(_) => Err(eyre!("Book does not exist {}", symbol)),
            },
            Err(e) => Err(eyre!(
                "Failed to parse snapshot data: {:?} - Data: {}",
                e,
                data
            )),
        }
    }

    pub fn apply_book_update(&mut self, symbol: &Symbol, data: Value) -> Result<()> {
        if let Ok(diff_depth) = serde_json::from_value::<DiffDepthUpdate>(data) {
            match self.books.entry(symbol.clone()) {
                Entry::Occupied(mut entry) => entry.get_mut().apply_update(diff_depth),
                Entry::Vacant(_) => Err(eyre!("Book does not exist {}", symbol)),
            }
        } else {
            Err(eyre!("Cannot parse DiffDepthUpdate"))
        }
    }

    pub fn apply_tob_update(&mut self, symbol: &str, data: Value) -> Result<()> {
        if let Ok(tob) = serde_json::from_value::<BookTickerUpdate>(data) {
            self.observer
                .read()
                .publish_many(&Arc::new(MarketDataEvent::TopOfBook {
                    symbol: symbol.to_uppercase().into(),
                    sequence_number: tob.update_id,
                    best_bid_price: tob.best_bid_price,
                    best_ask_price: tob.best_ask_price,
                    best_bid_quantity: tob.best_bid_quantity,
                    best_ask_quantity: tob.best_ask_quantity,
                }));
            Ok(())
        } else {
            Err(eyre!("Cannot parse BookTickerUpdate for {}", symbol))
        }
    }
    
    pub fn apply_aggregate_trade(&mut self, symbol: &str, data: Value) -> Result<()> {
        if let Ok(agg_trade) = serde_json::from_value::<AggregateTrade>(data) {
            self.observer
                .read()
                .publish_many(&Arc::new(MarketDataEvent::Trade {
                    symbol: symbol.to_uppercase().into(),
                    sequence_number: agg_trade.trade_time,
                    price: agg_trade.price,
                    quantity: agg_trade.quantity,
                }));
            Ok(())
        } else {
            Err(eyre!("Cannot parse AggregateTrade for {}", symbol))
        }
    }

    pub fn apply_update(&mut self, value: Value) -> Result<()> {
        if let Some(s) = value["stream"].as_str() {
            if let Some((symbol, stream_kind)) = s.split_once("@") {
                let symbol = &symbol.to_uppercase().into();
                match stream_kind {
                    "depth" => self.apply_book_update(symbol, value["data"].clone()),
                    "bookTicker" => self.apply_tob_update(symbol, value["data"].clone()),
                    "aggTrade" => self.apply_aggregate_trade(symbol, value["data"].clone()),
                    _ => Err(eyre!("Unknown stream type {}", stream_kind)),
                }
            } else {
                Err(eyre!("Unknown type of stream: {}", value))
            }
        } else {
            Err(eyre!("Failed to obtain stream: {}", value))
        }
    }
}
