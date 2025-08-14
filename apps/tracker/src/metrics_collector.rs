use std::{
    collections::{hash_map::Entry, HashMap},
    sync::Arc,
};

use chrono::{DateTime, Utc};
use eyre::{eyre, OptionExt};
use itertools::Itertools;
use parking_lot::RwLock;
use safe_math::safe;
use serde_json::json;
use symm_core::{
    core::{
        bits::{Amount, PriceType, Side, Symbol},
        decimal_ext::DecimalExt,
    },
    market_data::{order_book::order_book_manager::OrderBookManager, price_tracker::PriceTracker},
};

use crate::metrics_buffer::MetricsBuffer;

pub struct MetricsCollector<BookManager>
where
    BookManager: OrderBookManager,
{
    price_tracker: Arc<RwLock<PriceTracker>>,
    book_manager: Arc<RwLock<BookManager>>,
    symbols: Vec<Symbol>,
    labels: Vec<String>,
    thresholds: Vec<Amount>,
    aggregation_period: chrono::Duration,
    has_market_data: bool,
    metrics_buffer: Option<MetricsBuffer>,
}

impl<BookManager> MetricsCollector<BookManager>
where
    BookManager: OrderBookManager,
{
    pub fn new(
        price_tracker: Arc<RwLock<PriceTracker>>,
        book_manager: Arc<RwLock<BookManager>>,
        symbols: Vec<Symbol>,
        labels: Vec<impl Into<String>>,
        thresholds: Vec<Amount>,
        aggregation_period: chrono::Duration,
    ) -> Self {
        Self {
            price_tracker,
            book_manager,
            symbols,
            labels: labels.into_iter().map_into().collect_vec(),
            thresholds,
            aggregation_period,
            has_market_data: false,
            metrics_buffer: None,
        }
    }

    fn check_missing_prices_and_books(&mut self) -> eyre::Result<()> {
        let prices = self
            .price_tracker
            .read()
            .get_prices(PriceType::VolumeWeighted, &self.symbols);

        let liquidity =
            self.book_manager
                .read()
                .get_liquidity_levels(Side::Sell, 1, &self.symbols)?;

        let symbols_count = self.symbols.len();
        let book_count = liquidity.len();
        let missing_books = symbols_count - book_count;
        let missing_prices = prices.missing_symbols.len();

        tracing::info!(
            "Warm-up: {} TOB and {} Books missing out of {}",
            missing_prices,
            missing_books,
            symbols_count,
        );

        self.has_market_data = missing_prices == 0 && missing_books == 0;
        Ok(())
    }

    fn add_threshold(price: Amount, threshold: Amount) -> eyre::Result<(Amount, Amount)> {
        let res = (
            safe!(safe!(Amount::ONE - threshold) * price).ok_or_eyre("Math problem")?,
            safe!(safe!(Amount::ONE + threshold) * price).ok_or_eyre("Math problem")?,
        );

        Ok(res)
    }

    fn transpose_liquidity_map<'a>(
        half_bands: impl IntoIterator<Item = &'a HashMap<Symbol, Amount>>,
    ) -> HashMap<Symbol, Vec<Amount>> {
        let mut map = HashMap::new();
        for half_band in half_bands {
            for (symbol, &liquidity) in half_band {
                match map.entry(symbol.clone()) {
                    Entry::Vacant(vacant_entry) => {
                        vacant_entry.insert(vec![liquidity]);
                    }
                    Entry::Occupied(mut occupied_entry) => {
                        occupied_entry.get_mut().push(liquidity);
                    }
                }
            }
        }
        map
    }

    fn get_price_limits(
        &self,
        prices: &HashMap<Symbol, Amount>,
        threshold: Amount,
    ) -> eyre::Result<(HashMap<Symbol, Amount>, HashMap<Symbol, Amount>)> {
        let (price_bands, errors): (Vec<_>, Vec<_>) = prices
            .iter()
            .map(
                |(symbol, &price)| -> eyre::Result<(Symbol, (Amount, Amount))> {
                    let added = Self::add_threshold(price, threshold)?;
                    Ok((symbol.clone(), added))
                },
            )
            .partition_result();

        if !errors.is_empty() {
            Err(eyre!(
                "Failed to obtain price limits: {:?}",
                errors.iter().map(|err| format!("{:?}", err)).join("; ")
            ))?;
        }

        let bid_limits: HashMap<Symbol, Amount> = price_bands
            .iter()
            .map(|(symbol, (min_bid, _))| (symbol.clone(), *min_bid))
            .collect();

        let ask_limits: HashMap<Symbol, Amount> = price_bands
            .iter()
            .map(|(symbol, (_, max_ask))| (symbol.clone(), *max_ask))
            .collect();

        let (bid_liquidity, ask_liquidity) = (|book_manager: &BookManager| {
            (
                book_manager.get_liquidity(Side::Buy, &bid_limits),
                book_manager.get_liquidity(Side::Sell, &ask_limits),
            )
        })(&self.book_manager.read());

        Ok((bid_liquidity?, ask_liquidity?))
    }

    fn collect_liquidity_bands(&mut self) -> eyre::Result<()> {
        let prices = self
            .price_tracker
            .read()
            .get_prices(PriceType::VolumeWeighted, &self.symbols);

        if !prices.missing_symbols.is_empty() {
            self.has_market_data = false;
            Err(eyre!("Missing prices"))?;
        }

        let (liquidity_bands, errors): (Vec<_>, Vec<_>) = self
            .thresholds
            .iter()
            .map(|threshold| self.get_price_limits(&prices.prices, *threshold))
            .partition_result();

        if !errors.is_empty() {
            Err(eyre!(
                "Failed to obtain liquidity bands: {:?}",
                errors.iter().map(|err| format!("{:?}", err)).join("; ")
            ))?;
        }

        let bid_bands = Self::transpose_liquidity_map(liquidity_bands.iter().map(|band| &band.0));
        let ask_bands = Self::transpose_liquidity_map(liquidity_bands.iter().map(|band| &band.1));

        let metrics_buffer = self
            .metrics_buffer
            .get_or_insert_with(|| MetricsBuffer::new(self.aggregation_period));

        if metrics_buffer.ingest(&self.labels, bid_bands, ask_bands)? {
            self.metrics_buffer = None;
        }

        Ok(())
    }

    pub fn collect_metrics(&mut self) -> eyre::Result<()> {
        if !self.has_market_data {
            self.check_missing_prices_and_books()?;
            if !self.has_market_data {
                return Ok(());
            }
        }

        self.collect_liquidity_bands()?;

        Ok(())
    }
}
