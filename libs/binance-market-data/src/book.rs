use std::{
    collections::{hash_map::Entry, HashMap, VecDeque},
    sync::Arc,
};

use binance_sdk::spot::{
    rest_api::DepthResponse,
    websocket_streams::{BookTickerResponse, DiffBookDepthResponse},
};
use chrono::{DateTime, Duration, Utc};
use eyre::{eyre, OptionExt, Report, Result};
use itertools::Itertools;
use parking_lot::RwLock as AtomicLock;
use serde::{Deserialize, Serialize};
use symm_core::{
    core::{
        bits::{Amount, PricePointEntry, Symbol},
        functional::{MultiObserver, PublishMany},
    },
    market_data::market_data_connector::MarketDataEvent,
};
use tokio::sync::mpsc::UnboundedSender;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PriceLevel(Amount, Amount);

#[derive(Debug)]
struct DepthSnapshot {
    last_update_id: u64,
    bids: Vec<PriceLevel>,
    asks: Vec<PriceLevel>,
}

#[derive(Debug)]
struct DiffDepthUpdate {
    first_update_id_in_event: u64,
    final_update_id_in_event: u64,
    bids: Vec<PriceLevel>,
    asks: Vec<PriceLevel>,
}

#[derive(Debug)]
struct BookTickerUpdate {
    best_bid_price: Amount,
    best_bid_quantity: Amount,
    best_ask_price: Amount,
    best_ask_quantity: Amount,
    symbol: String,
    update_id: u64,
}

impl TryFrom<Vec<String>> for PriceLevel {
    type Error = Report;

    fn try_from(value: Vec<String>) -> std::result::Result<Self, Self::Error> {
        let (good, bad): (Vec<_>, Vec<_>) = value
            .into_iter()
            .map(|x| x.parse::<Amount>())
            .partition_result();

        if !bad.is_empty() {
            return Err(eyre!(
                "Cannot parse price level: {}",
                bad.into_iter().map(|e| format!("{:?}", e)).join(";")
            ));
        }

        let (a, b) = good
            .into_iter()
            .collect_tuple()
            .ok_or_eyre("Expected two elements")?;

        Ok(PriceLevel(a, b))
    }
}

fn price_levels_try_from(value: Vec<Vec<String>>) -> Result<Vec<PriceLevel>> {
    let (good, bad): (Vec<_>, Vec<_>) = value
        .into_iter()
        .map(|val| PriceLevel::try_from(val))
        .partition_result();

    if !bad.is_empty() {
        return Err(eyre!(
            "Cannot parse price level: {}",
            bad.into_iter().map(|e| format!("{:?}", e)).join(";")
        ));
    }

    let levels = good.into_iter().collect_vec();
    Ok(levels)
}

impl TryFrom<DepthResponse> for DepthSnapshot {
    type Error = Report;

    fn try_from(value: DepthResponse) -> std::result::Result<Self, Self::Error> {
        Ok(Self {
            last_update_id: value.last_update_id.ok_or_eyre("Missing last update ID")? as u64,
            bids: price_levels_try_from(value.bids.ok_or_eyre("Misssing bids")?)?,
            asks: price_levels_try_from(value.asks.ok_or_eyre("Misssing asks")?)?,
        })
    }
}

impl TryFrom<DiffBookDepthResponse> for DiffDepthUpdate {
    type Error = Report;

    fn try_from(value: DiffBookDepthResponse) -> std::result::Result<Self, Self::Error> {
        Ok(Self {
            first_update_id_in_event: value.u_uppercase.ok_or_eyre("Missing first update id")?
                as u64,
            final_update_id_in_event: value.u.ok_or_eyre("Missing final update id")? as u64,
            bids: price_levels_try_from(value.b.ok_or_eyre("Missing bids")?)?,
            asks: price_levels_try_from(value.a.ok_or_eyre("Missing asks")?)?,
        })
    }
}

impl TryFrom<BookTickerResponse> for BookTickerUpdate {
    type Error = Report;

    fn try_from(value: BookTickerResponse) -> Result<Self, Self::Error> {
        Ok(Self {
            best_bid_price: value.b.ok_or_eyre("Missing bid price")?.parse()?,
            best_bid_quantity: value
                .b_uppercase
                .ok_or_eyre("Missing bid quantity")?
                .parse()?,
            best_ask_price: value.a.ok_or_eyre("Missing ask price")?.parse()?,
            best_ask_quantity: value
                .a_uppercase
                .ok_or_eyre("Missing ask quantity")?
                .parse()?,
            symbol: value.s.ok_or_eyre("Missing symbol")?,
            update_id: value.u.ok_or_eyre("Missing update id")? as u64,
        })
    }
}

struct SnapshotRequest {
    symbol: Symbol,
    timestamp: DateTime<Utc>,
}

struct Book {
    observer: Arc<AtomicLock<MultiObserver<Arc<MarketDataEvent>>>>,
    symbol: Symbol,
    snapshot_tx: UnboundedSender<Symbol>,
    pending_updates: VecDeque<DiffDepthUpdate>,
    last_update_id: Option<u64>,
    snapshot_requested: bool,
    last_update_timestamp: DateTime<Utc>,
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
            last_update_timestamp: Utc::now(),
        }
    }

    fn check_stale(&mut self, timeout: Duration) -> Result<()> {
        if !self.snapshot_requested {
            let now = Utc::now();
            let age = now - self.last_update_timestamp;
            if timeout < age {
                tracing::debug!(
                    "Check stale for {} is stale and snapshot is needed (now: {}, last_update_timestamp: {}, last_update_id: {:?})",
                    self.symbol,
                    now,
                    self.last_update_timestamp,
                    self.last_update_id,
                );
                self.request_snapshot()?;
            }
        }
        Ok(())
    }

    fn request_snapshot(&mut self) -> Result<()> {
        tracing::info!("Requesting snapshot for {}", self.symbol);
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
        tracing::info!(
            "(binance-book) Snapshot received for {} and will overwrite book (lastUpdateId: {})",
            self.symbol,
            snapshot.last_update_id
        );
        self.last_update_id = Some(snapshot.last_update_id);
        self.last_update_timestamp = Utc::now();
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
                    if !self.snapshot_requested {
                        tracing::debug!(
                            "DiffDepthUpdate for {} is ahead and snapshot is needed (U: {}, u: {} vs last_id: {})",
                            self.symbol,
                            diff_depth.first_update_id_in_event,
                            diff_depth.final_update_id_in_event,
                            last_id,
                        );
                        self.request_snapshot()?;
                    }
                    self.pending_updates.push_back(diff_depth);
                } else if diff_depth.final_update_id_in_event < last_id {
                    tracing::trace!(
                            "DiffDepthUpdate for {} is old and will be ignored (U: {}, u: {} vs last_id: {})",
                            self.symbol,
                            diff_depth.first_update_id_in_event,
                            diff_depth.final_update_id_in_event,
                            last_id,
                        );
                } else {
                    tracing::trace!(
                        "DiffDepthUpdate for {} is new and will be applied (U: {}, u: {})",
                        self.symbol,
                        diff_depth.first_update_id_in_event,
                        diff_depth.final_update_id_in_event,
                    );
                    self.last_update_id = Some(diff_depth.final_update_id_in_event);
                    self.last_update_timestamp = Utc::now();
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
                if !self.snapshot_requested {
                    tracing::debug!(
                    "DiffDepthUpdate for {} empty book and snapshot is needed (U: {}, u: {} vs last_id: None)",
                    self.symbol,
                    diff_depth.first_update_id_in_event,
                    diff_depth.final_update_id_in_event,
                );
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

    pub fn check_stale(&mut self, timeout: Duration) {
        for book in self.books.values_mut() {
            book.check_stale(timeout);
        }
    }

    pub fn add_book(
        &mut self,
        symbol: &Symbol,
        snapshot_tx: UnboundedSender<Symbol>,
        request_snapshot: bool,
    ) -> Result<()> {
        match self.books.entry(symbol.clone()) {
            Entry::Vacant(entry) => {
                let book = entry.insert(Book::new_with_observer(
                    symbol.clone(),
                    snapshot_tx.clone(),
                    self.observer.clone(),
                ));
                if request_snapshot {
                    return book.request_snapshot();
                }
                Ok(())
            }
            Entry::Occupied(_) => Err(eyre!("Book already exists {}", symbol)),
        }
    }

    pub fn apply_snapshot(&mut self, symbol: &Symbol, data: DepthResponse) -> Result<()> {
        match DepthSnapshot::try_from(data) {
            Ok(snapshot) => match self.books.entry(symbol.clone()) {
                Entry::Occupied(mut entry) => entry.get_mut().apply_snapshot(snapshot),
                Entry::Vacant(_) => Err(eyre!("Book does not exist {}", symbol)),
            },
            Err(e) => Err(eyre!("Failed to parse snapshot data: {:?}", e)),
        }
    }

    pub fn apply_book_update(
        &mut self,
        symbol: &Symbol,
        data: DiffBookDepthResponse,
    ) -> Result<()> {
        match DiffDepthUpdate::try_from(data) {
            Ok(diff_depth) => match self.books.entry(symbol.clone()) {
                Entry::Occupied(mut entry) => entry.get_mut().apply_update(diff_depth),
                Entry::Vacant(_) => Err(eyre!("Book does not exist {}", symbol)),
            },
            Err(err) => Err(eyre!("Cannot parse DiffDepthUpdate {:?}", err)),
        }
    }

    pub fn apply_tob_update(&mut self, symbol: &str, data: BookTickerResponse) -> Result<()> {
        match BookTickerUpdate::try_from(data) {
            Ok(tob) => {
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
            }
            Err(err) => Err(eyre!("Cannot parse BookTickerResponse {:?}", err)),
        }
    }
}
