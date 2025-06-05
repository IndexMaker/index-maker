use std::collections::{hash_map::Entry, HashMap};

use eyre::{eyre, Result};
use index_maker::core::bits::{Amount, Symbol};
use rand::Rng;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::mpsc::UnboundedSender;

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
    best_bid_price: Amount,
    #[serde(rename = "B")]
    best_bid_quantity: Amount,
    #[serde(rename = "a")]
    best_ask_price: Amount,
    #[serde(rename = "A")]
    best_ask_quantity: Amount,
    #[serde(rename = "s")]
    symbol: String,
    #[serde(rename = "u")]
    update_id: u64,
}

pub struct Book {
    symbol: Symbol,
    snapshot_tx: UnboundedSender<Symbol>,
    last_update_id: Option<u64>,
    needs_snapshot: bool,
}

impl Book {
    pub fn new(symbol: Symbol, snapshot_tx: UnboundedSender<Symbol>) -> Self {
        Book {
            snapshot_tx,
            symbol,
            last_update_id: None,
            needs_snapshot: false,
        }
    }

    pub fn apply_snapshot(&mut self, data: &str) {
        match serde_json::from_str::<DepthSnapshot>(data) {
            Ok(snapshot) => {
                println!(
                    "Snapshot received (symbol: {}, lastUpdateId: {}). Overwriting book.",
                    self.symbol, snapshot.last_update_id
                );
                self.last_update_id = Some(snapshot.last_update_id);
                self.needs_snapshot = false;
            }
            Err(e) => {
                eprintln!("Failed to parse snapshot data: {:?} - Data: {}", e, data);
                self.needs_snapshot = true;
                self.snapshot_tx.send(self.symbol.clone());
            }
        }
    }

    pub fn apply_update(&mut self, data: Value) {
        if rand::rng().random_range(0.0..1.0) < 0.2 {
            println!("Simulating drop of a DiffDepthUpdate message.");
            return;
        }
        if let Ok(diff_depth) = serde_json::from_value::<DiffDepthUpdate>(data) {
            match self.last_update_id {
                Some(last_id) => {
                    if diff_depth.first_update_id_in_event <= last_id + 1
                        && diff_depth.final_update_id_in_event >= last_id
                    {
                        println!(
                            "DiffDepthUpdate received (U: {}, u: {}). Applying update to {}.",
                            diff_depth.first_update_id_in_event,
                            diff_depth.final_update_id_in_event,
                            self.symbol
                        );
                        self.last_update_id = Some(diff_depth.final_update_id_in_event);
                    } else {
                        println!(
                            "DiffDepthUpdate out of sync (U: {}, u: {} vs last_id: {}). Requesting new snapshot for {}.",
                            diff_depth.first_update_id_in_event,
                            diff_depth.final_update_id_in_event,
                            last_id,
                            self.symbol
                        );
                        self.needs_snapshot = true;
                        self.snapshot_tx.send(self.symbol.clone());
                    }
                }
                None => {
                    println!(
                        "No snapshot received yet. Cannot apply DiffDepthUpdate (U: {}, u: {}). Requesting snapshot for {}.",
                        diff_depth.first_update_id_in_event, diff_depth.final_update_id_in_event, self.symbol
                    );
                    self.needs_snapshot = true;
                    self.snapshot_tx.send(self.symbol.clone());
                }
            }
        } else {
            eprintln!("Cannot parse DiffDepthUpdate");
        }
    }
}

pub struct Books {
    books: HashMap<Symbol, Book>,
}

impl Books {
    pub fn new() -> Self {
        Self {
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
                entry.insert(Book::new(symbol.clone(), snapshot_tx.clone()));
                Ok(())
            }
            Entry::Occupied(_) => Err(eyre!("Book already exists {}", symbol)),
        }
    }

    pub fn apply_snapshot(&mut self, symbol: &Symbol, data: &str) -> Result<()> {
        match self.books.entry(symbol.clone()) {
            Entry::Occupied(mut entry) => {
                entry.get_mut().apply_snapshot(data);
                Ok(())
            }
            Entry::Vacant(_) => Err(eyre!("Book does not exist {}", symbol)),
        }
    }

    pub fn apply_book_update(&mut self, symbol: &Symbol, data: Value) -> Result<()> {
        match self.books.entry(symbol.clone()) {
            Entry::Occupied(mut entry) => {
                entry.get_mut().apply_update(data);
                Ok(())
            }
            Entry::Vacant(_) => Err(eyre!("Book does not exist {}", symbol)),
        }
    }

    pub fn apply_tob_update(&mut self, symbol: &str, data: Value) -> Result<()> {
        if let Ok(_) = serde_json::from_value::<BookTickerUpdate>(data) {
            // do nothing, too many ticks to print
            Ok(())
        } else {
            Err(eyre!("Cannot parse BookTickerUpdate for {}", symbol))
        }
    }

    pub fn apply_update(&mut self, value: Value) -> Result<()> {
        if let Some(s) = value["stream"].as_str() {
            if let Some((symbol, stream_kind)) = s.split_once("@") {
                let symbol = &symbol.to_uppercase().into();
                match stream_kind {
                    "depth" => self.apply_book_update(symbol, value["data"].clone()),
                    "bookTicker" => self.apply_tob_update(symbol, value["data"].clone()),
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
