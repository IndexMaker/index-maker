use std::{
    collections::{hash_map::Entry, HashMap},
    sync::Arc,
};

use clap::error;
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

use crate::metrics_buffer::{MetricsBuffer, MetricsWriterMode};

pub const LIQUIDITY_TOLERANCE: Amount = Amount::from_parts(1, 0, 0, false, 8);

pub struct MetricsCollector<BookManager>
where
    BookManager: OrderBookManager,
{
    price_tracker: Arc<RwLock<PriceTracker>>,
    book_manager: Arc<RwLock<BookManager>>,
    symbols: Vec<Symbol>,
    labels: Vec<String>,
    thresholds: Vec<Amount>,
    mode: MetricsWriterMode,
    aggregation_period: chrono::Duration,
    has_market_data: bool,
    metrics_buffer: Option<MetricsBuffer>,
    base_asset_by_symbol: HashMap<Symbol, Symbol>,
    get_flush_path_fn: Box<dyn Fn() -> String + Send + Sync>,
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
        mode: MetricsWriterMode,
        aggregation_period: chrono::Duration,
        base_asset_by_symbol: HashMap<Symbol, Symbol>,
        flush_path_fn: impl Fn() -> String + Send + Sync + 'static,
    ) -> Self {
        Self {
            price_tracker,
            book_manager,
            symbols,
            labels: labels.into_iter().map_into().collect_vec(),
            thresholds,
            mode,
            aggregation_period,
            has_market_data: false,
            metrics_buffer: None,
            base_asset_by_symbol,
            get_flush_path_fn: Box::new(flush_path_fn),
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

        self.has_market_data = missing_prices < (symbols_count / 4) && missing_books == 0;
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

    fn cumulative_to_differencial_labels(labels: &Vec<String>) -> Vec<String> {
        labels.windows(2).map(|x| x.join("/")).collect_vec()
    }

    fn cumulative_to_differencial_liqiuidity_map(
        price_limits: HashMap<Symbol, Vec<Amount>>,
        bands: HashMap<Symbol, Vec<Amount>>,
    ) -> HashMap<Symbol, Vec<Option<Amount>>> {
        bands
            .into_iter()
            .map(|(symbol, bands)| {
                if let Some(price_limits) = price_limits.get(&symbol) {
                    let bands = bands
                        .iter()
                        .zip(price_limits)
                        // Convert liquidity to USD
                        .map(|(&band_liquidity, &price)| safe!(band_liquidity * price))
                        .map(|x| {
                            if x.is_none() {
                                tracing::warn!(%symbol, "Price multiplication error");
                            }
                            x
                        })
                        .tuple_windows()
                        // Difference between higer and lower band
                        .map(|(lower_band, higher_band)| safe!(higher_band - lower_band?))
                        .map(|x| {
                            if x.is_none() {
                                tracing::warn!(%symbol, "High-Low subtraction error");
                            }
                            x
                        })
                        // Sanitize removing non-positive values
                        .map(|liquidity_value| {
                            liquidity_value.and_then(|x| (LIQUIDITY_TOLERANCE < x).then_some(x))
                        })
                        .collect_vec();
                    (symbol, bands)
                } else {
                    let bands = bands
                        .iter()
                        .tuple_windows()
                        // This will happen if price tracker and book manager
                        // are out of sync and one has symbol, while other
                        // doesn't. We just return None, and that will be
                        // reported as missing value.
                        .map(|(_, _)| None)
                        .collect_vec();
                    tracing::warn!(%symbol, "Missing price limits");
                    (symbol, bands)
                }
            })
            .collect()
    }

    fn get_price_limits(
        &self,
        prices: &HashMap<Symbol, Amount>,
        threshold: Amount,
    ) -> eyre::Result<(
        HashMap<Symbol, Amount>,
        HashMap<Symbol, Amount>,
        HashMap<Symbol, Amount>,
        HashMap<Symbol, Amount>,
    )> {
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

        Ok((bid_limits, bid_liquidity?, ask_limits, ask_liquidity?))
    }

    fn collect_liquidity_bands(&mut self) -> eyre::Result<()> {
        // First obtain prices at the tob of the book using volume weighted approach
        let prices = {
            let mut prices = self
                .price_tracker
                .read()
                .get_prices(PriceType::VolumeWeighted, &self.symbols);

            if !prices.missing_symbols.is_empty() {
                tracing::warn!(
                    missing_symbols = %json!(prices.missing_symbols),
                    "Missing symbols - will try to use Best Book Offer");

                let best_book_offer = self
                    .book_manager
                    .read()
                    .get_top_level(&prices.missing_symbols)?;

                let (best_book_offers, errors): (Vec<_>, Vec<_>) = best_book_offer
                    .into_iter()
                    .map(|(symbol, bbo)| -> eyre::Result<(Symbol, Amount)> {
                        Ok((
                            symbol,
                            bbo.volume_weighted()
                                .ok_or_eyre("Failed to compute volume weighted best book offer")?,
                        ))
                    })
                    .partition_result();

                if !errors.is_empty() {
                    self.has_market_data = false;

                    Err(eyre!(
                        "Missing prices: {}",
                        errors.into_iter().map(|err| format!("{:?}", err)).join(";")
                    ))?;
                }

                prices.prices.extend(best_book_offers);
            }

            prices.prices
        };

        // Map list of thresholds into liquidity bands, for each band we will have map Symbol => Liquidity
        let (liquidity_bands, errors): (Vec<_>, Vec<_>) = self
            .thresholds
            .iter()
            .map(|threshold| self.get_price_limits(&prices, *threshold))
            .partition_result();

        if !errors.is_empty() {
            Err(eyre!(
                "Failed to obtain liquidity bands: {:?}",
                errors.iter().map(|err| format!("{:?}", err)).join("; ")
            ))?;
        }

        // Transpose vector of Symbol => Liquidity map into map Symbol => vector of liquidity,
        // i.e. convert vector of maps into map of vectors
        let bid_limits = Self::transpose_liquidity_map(liquidity_bands.iter().map(|band| &band.0));
        let bid_bands = Self::transpose_liquidity_map(liquidity_bands.iter().map(|band| &band.1));
        let ask_limits = Self::transpose_liquidity_map(liquidity_bands.iter().map(|band| &band.2));
        let ask_bands = Self::transpose_liquidity_map(liquidity_bands.iter().map(|band| &band.3));

        // Transform from cumulative form to differencial form,
        // i.e. from form (0..10), (0..20), ..(0..500) to form (10..20), (20..30), ..(400..500)
        // We transform both data and labels.
        let labels = Self::cumulative_to_differencial_labels(&self.labels);
        let bid_bands = Self::cumulative_to_differencial_liqiuidity_map(bid_limits, bid_bands);
        let ask_bands = Self::cumulative_to_differencial_liqiuidity_map(ask_limits, ask_bands);

        tracing::info!("Collected bands for {} symbols", bid_bands.len());

        // Send collected bands into metrics collection buffer, which buffers one period,
        // after which we take the stats and flush them into CSV file.
        let should_flush = self
            .metrics_buffer
            .get_or_insert_with(|| MetricsBuffer::new(labels, self.mode, self.aggregation_period))
            .ingest(bid_bands, ask_bands)?;

        // Metrics buffer is full we need to flush it, i.e. collection time interval
        // is fully covered by metrics buffer, and we can calcualte stats, write them
        // to CSV, and start over with new metrics buffer.
        if should_flush {
            let flush_path = (*self.get_flush_path_fn)();

            self.metrics_buffer
                .take()
                .unwrap()
                .flush_stats(&self.base_asset_by_symbol, &flush_path)?;
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
