use chrono::{Timelike, Utc};
use csv::ReaderBuilder;
use eyre::Result;
use index_maker::app::market_data::MarketDataConfig;
use parking_lot::RwLock;
use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use safe_math::safe;
use std::fs::OpenOptions;
use std::{collections::HashMap, sync::Arc, time::Duration};
use symm_core::core::bits::{Amount, Side, Symbol};
use symm_core::{
    core::{
        bits::PriceType, decimal_ext::DecimalExt, functional::IntoObservableManyArc,
        logging::log_init,
    },
    init_log,
    market_data::{
        market_data_connector::{MarketDataEvent, Subscription},
        order_book::order_book_manager::PricePointBookManager,
        price_tracker::PriceTracker,
    },
};
use tokio::signal::unix::{signal, SignalKind};
use tokio::sync::mpsc::unbounded_channel;
use tokio::time::{sleep, Instant};
#[derive(Default, Clone)]
struct Stats {
    n: usize,
    missing: usize,
    mean: Decimal,
    std: Decimal,
    min: Decimal,
    p05: Decimal,
    p25: Decimal,
    p50: Decimal,
    p75: Decimal,
    p95: Decimal,
    max: Decimal,
}

// CUMULATIVE bounds:  0.1,0.2,0.3,0.4,0.5,0.75,1,1.5,2,3,4,5%
const BUCKETS: &[Amount] = &[
    dec!(0.001),
    dec!(0.002),
    dec!(0.003),
    dec!(0.004),
    dec!(0.005),
    dec!(0.0075),
    dec!(0.01),
    dec!(0.015),
    dec!(0.02),
    dec!(0.03),
    dec!(0.04),
    dec!(0.05),
];

fn nearest_rank(sorted: &[Decimal], q: Decimal) -> Decimal {
    use rust_decimal::prelude::ToPrimitive;
    if sorted.is_empty() {
        return Decimal::ZERO;
    }
    // nearest‑rank (Hyndman & Fan R1): k = ceil(q * n), 1-based, clamp
    let n = sorted.len() as i64;
    let k_amount = safe!(q * Amount::from(n as i64))
        .unwrap_or(Amount::ONE)
        .ceil();
    let k = k_amount.to_i64().unwrap_or(1).clamp(1, n) - 1;
    sorted[k as usize]
}

fn sqrt_decimal(x: Decimal) -> Decimal {
    if x <= Decimal::ZERO {
        return Decimal::ZERO;
    }
    let two = dec!(2);
    let mut guess = x / two;
    for _ in 0..20 {
        guess = (guess + (x / guess)) / two;
    }
    guess
}

fn compute_stats(mut xs: Vec<Decimal>, total_snapshots: usize) -> Stats {
    if xs.is_empty() {
        return Stats {
            n: 0,
            missing: total_snapshots,
            ..Default::default()
        };
    }
    xs.sort_unstable();
    let n = xs.len();
    let missing = total_snapshots.saturating_sub(n);
    // mean & std
    let sum: Decimal = xs.iter().copied().sum();
    let mean = sum / Amount::from(n as i64);
    let mut sumsq = Decimal::ZERO;
    for &v in &xs {
        sumsq += v * v;
    }
    let var = (sumsq / Decimal::from(n as u32)) - (mean * mean);
    let std = sqrt_decimal(var.max(Decimal::ZERO));

    Stats {
        n,
        missing,
        mean,
        std,
        min: xs.first().copied().unwrap_or(Decimal::ZERO),
        p05: nearest_rank(&xs, dec!(0.05)),
        p25: nearest_rank(&xs, dec!(0.25)),
        p50: nearest_rank(&xs, dec!(0.50)),
        p75: nearest_rank(&xs, dec!(0.75)),
        p95: nearest_rank(&xs, dec!(0.95)),
        max: xs.last().copied().unwrap_or(Decimal::ZERO),
    }
}

fn fmt2(d: Decimal) -> String {
    d.round_dp(2).to_string()
}

#[derive(Debug, Clone)]
struct SymbolRow {
    symbol: String,
    base_asset: String,
}

fn stat_suffixes() -> [&'static str; 11] {
    [
        "Samples", "Missing", "Mean", "Std", "Min", "P05", "P25", "P50", "P75", "P95", "Max",
    ]
}

fn to_bps(d: Decimal) -> i32 {
    use rust_decimal::prelude::ToPrimitive;
    ((d * dec!(10000)).round()).to_i32().unwrap_or(0)
}

fn range_edges_bps() -> Vec<(i32, i32)> {
    BUCKETS
        .windows(2)
        .map(|w| (to_bps(w[0]), to_bps(w[1])))
        .collect()
}
fn make_headers() -> Vec<String> {
    let mut h = vec![
        "TimestampStart".into(),
        "TimestampEnd".into(),
        "Symbol".into(),
        "Base Asset".into(),
    ];
    for side in ["Bid", "Ask"] {
        for (lo, hi) in range_edges_bps() {
            for suf in stat_suffixes() {
                // No space before suffix: Bid(50/70)Min
                h.push(format!("{side}({lo}/{hi}){suf}"));
            }
        }
    }
    h
}

async fn load_symbols_from_csv(path: &str) -> Result<Vec<SymbolRow>> {
    let mut rdr = ReaderBuilder::new()
        .has_headers(true)
        .trim(csv::Trim::All)
        .from_path(path)?;
    let mut out = Vec::new();
    for rec in rdr.records() {
        let r = rec?;
        if r.len() < 3 {
            continue;
        }
        out.push(SymbolRow {
            symbol: r[0].to_string(),
            base_asset: r[1].to_string(),
        });
    }
    Ok(out)
}

/// Take one snapshot:
/// - Get VWAP prices per symbol
/// - For each side & threshold: build price limits and get cumulative liquidity
/// - Diff cumulatives to range buckets
/// - Convert to USD by multiplying by price (same snapshot)
///
/// Returns symbol -> { "Buy 0.1-0.2%": Amount(USD), ... }
fn take_liquidity_snapshot_usd(
    symbols: &[Symbol],
    price_tracker: &Arc<RwLock<PriceTracker>>,
    obm: &Arc<RwLock<PricePointBookManager>>,
    _cap_by_levels: Option<usize>,
) -> eyre::Result<HashMap<Symbol, HashMap<String, Amount>>> {
    use symm_core::market_data::order_book::order_book_manager::OrderBookManager;

    let pt = price_tracker.read();
    let bm = obm.read();

    // USD conversion prices (prefer VWAP, fallback to midpoint)
    let mut usd_px_all = pt.get_prices(PriceType::VolumeWeighted, symbols).prices;
    if usd_px_all.len() < symbols.len() {
        for s in symbols {
            if usd_px_all.contains_key(s) {
                continue;
            }
            if let Some(book) = bm.get_order_book(s) {
                let bb = book.get_entries(Side::Buy, 1).first().map(|e| e.price);
                let ba = book.get_entries(Side::Sell, 1).first().map(|e| e.price);
                if let (Some(b), Some(a)) = (bb, ba) {
                    // safe midpoint; if your Amount is Decimal-compatible this is fine
                    usd_px_all.insert(s.clone(), (b + a) / dec!(2));
                }
            }
        }
    }

    // keep only positive USD prices
    let mut usd_px: HashMap<Symbol, Amount> = HashMap::new();
    for s in symbols {
        if let Some(px) = usd_px_all.get(s) {
            if *px > Amount::ZERO {
                usd_px.insert(s.clone(), *px);
            }
        }
    }

    // helper: base prices per side (BestAsk for Buy, BestBid for Sell), fallback to OBM top
    let mut base_prices_for_side = |side: Side| -> HashMap<Symbol, Amount> {
        let price_type = match side {
            Side::Buy => PriceType::BestAsk,
            Side::Sell => PriceType::BestBid,
        };
        let mut px = pt.get_prices(price_type, symbols).prices;

        for s in symbols {
            if px.contains_key(s) {
                continue;
            }
            if let Some(book) = bm.get_order_book(s) {
                match side {
                    Side::Buy => {
                        if let Some(ask) = book.get_entries(Side::Sell, 1).first() {
                            px.insert(s.clone(), ask.price);
                        }
                    }
                    Side::Sell => {
                        if let Some(bid) = book.get_entries(Side::Buy, 1).first() {
                            px.insert(s.clone(), bid.price);
                        }
                    }
                }
            }
        }

        // keep positive
        px.into_iter().filter(|(_, p)| *p > Amount::ZERO).collect()
    };

    let mut out: HashMap<Symbol, HashMap<String, Amount>> = HashMap::new();

    // Use global BUCKETS and build labels in BPS so they match make_headers()
    for &side in &[Side::Buy, Side::Sell] {
        let base = base_prices_for_side(side);
        if base.is_empty() {
            continue;
        }

        let opp = side.opposite_side();

        // cumulative per threshold
        let mut cum: Vec<HashMap<Symbol, Amount>> = Vec::with_capacity(BUCKETS.len());

        for &thr in BUCKETS.iter() {
            let factor = match side {
                Side::Buy => Amount::ONE + thr,  // buy up to ask*(1+thr)
                Side::Sell => Amount::ONE - thr, // sell down to bid*(1-thr)
            };

            let mut limits = HashMap::with_capacity(base.len());
            for (s, p0) in base.iter() {
                limits.insert(s.clone(), *p0 * factor);
            }

            let qty_map = match OrderBookManager::get_liquidity(&*bm, opp, &limits) {
                Ok(m) => m,
                Err(e) => {
                    tracing::warn!("get_liquidity failed for side={:?}: {:?}", side, e);
                    HashMap::new()
                }
            };

            // sanitize negatives and keep requested symbols only
            let mut cleaned = HashMap::with_capacity(qty_map.len());
            for (s, q) in qty_map {
                if q > Amount::ZERO && limits.contains_key(&s) {
                    cleaned.insert(s, q);
                }
            }

            cum.push(cleaned);
        }

        if cum.len() < 2 {
            continue;
        }

        for s in symbols {
            let Some(conv_px) = usd_px.get(s) else {
                continue;
            };

            for (i_lo, i_hi) in (0..BUCKETS.len() - 1).zip(1..BUCKETS.len()) {
                let q_lo = *cum[i_lo].get(s).unwrap_or(&Amount::ZERO);
                let q_hi = *cum[i_hi].get(s).unwrap_or(&Amount::ZERO);
                if q_hi <= q_lo {
                    continue;
                }

                let range_qty = q_hi - q_lo;
                if range_qty <= Amount::ZERO {
                    continue;
                }

                let usd = range_qty * *conv_px;

                // Build the EXACT key format your headers/stats expect
                let lo_bps = to_bps(BUCKETS[i_lo]);
                let hi_bps = to_bps(BUCKETS[i_hi]);
                let side_tag = match side {
                    Side::Buy => "Bid",
                    Side::Sell => "Ask",
                }; // important!
                let col = format!("{side_tag}({lo_bps}/{hi_bps})");

                *out.entry(s.clone())
                    .or_default()
                    .entry(col)
                    .or_insert(Amount::ZERO) += usd;
            }
        }
    }

    Ok(out)
}

/// One 5‑minute batch: 60 snapshots, every 5 seconds.
/// Returns symbol -> column -> **sum** of USD values across the 60 snapshots.
async fn run_5min_batch_usd_stats(
    symbols_rows: &[SymbolRow],
    price_tracker: Arc<RwLock<PriceTracker>>,
    obm: Arc<RwLock<PricePointBookManager>>,
    base_by_symbol: &HashMap<Symbol, String>,
) -> eyre::Result<Vec<HashMap<String, String>>> {
    use tokio::time::{sleep, Duration};

    let total_snaps = 60usize;
    let symbols_only: Vec<Symbol> = symbols_rows
        .iter()
        .map(|r| Symbol::from(r.symbol.as_str()))
        .collect();

    let ts_start = chrono::Utc::now();

    // symbol -> "Bid(lo/hi)" | "Ask(lo/hi)" -> Vec<Decimal> USD samples
    let mut samples: HashMap<Symbol, HashMap<String, Vec<Decimal>>> = HashMap::new();

    for i in 0..total_snaps {
        tracing::info!(target:"liquidity_tracker","Snapshot {}/{}", i + 1, total_snaps);

        match take_liquidity_snapshot_usd(&symbols_only, &price_tracker, &obm, None) {
            Ok(snap) => {
                for (sym, cols) in snap {
                    let entry = samples.entry(sym).or_default();
                    for (k, v) in cols {
                        if v > Decimal::ZERO {
                            entry.entry(k).or_default().push(v);
                        }
                    }
                }
            }
            Err(e) => {
                tracing::warn!("snapshot failed (skipping this tick): {:?}", e);
            }
        }

        sleep(Duration::from_secs(5)).await;
    }

    let ts_end = chrono::Utc::now();

    // Build rows
    let mut rows: Vec<HashMap<String, String>> = Vec::new();
    let labels_bps = range_edges_bps(); // Vec<(lo_bps, hi_bps)>

    for sym in &symbols_only {
        let mut row = HashMap::new();
        row.insert("TimestampStart".into(), ts_start.timestamp().to_string()); // <-- match header
        row.insert("TimestampEnd".into(), ts_end.timestamp().to_string()); // <-- match header
        row.insert("Symbol".into(), sym.to_string());
        row.insert(
            "Base Asset".into(),
            base_by_symbol.get(sym).cloned().unwrap_or_default(),
        );

        let per_col = samples.get(sym).cloned().unwrap_or_default();

        for side_prefix in ["Bid", "Ask"] {
            for (lo, hi) in &labels_bps {
                let base_key = format!("{side_prefix}({lo}/{hi})");
                let vecv = per_col.get(&base_key).cloned().unwrap_or_default();
                let st = compute_stats(vecv, total_snaps);

                // EXACT suffix names expected by headers
                row.insert(format!("{base_key}Samples"), st.n.to_string());
                row.insert(format!("{base_key}Missing"), st.missing.to_string());
                row.insert(format!("{base_key}Mean"), fmt2(st.mean));
                row.insert(format!("{base_key}Std"), fmt2(st.std));
                row.insert(format!("{base_key}Min"), fmt2(st.min));
                row.insert(format!("{base_key}P05"), fmt2(st.p05));
                row.insert(format!("{base_key}P25"), fmt2(st.p25));
                row.insert(format!("{base_key}P50"), fmt2(st.p50));
                row.insert(format!("{base_key}P75"), fmt2(st.p75));
                row.insert(format!("{base_key}P95"), fmt2(st.p95));
                row.insert(format!("{base_key}Max"), fmt2(st.max));
            }
        }

        rows.push(row);
    }

    Ok(rows)
}

fn write_csv_append(
    path: &str,
    headers: &[String],
    rows: &[HashMap<String, String>],
) -> eyre::Result<()> {
    // open in append mode, create if missing
    let mut file = OpenOptions::new().create(true).append(true).open(path)?;

    // if file length is zero, we need to write headers once
    let write_header = file.metadata()?.len() == 0;

    // IMPORTANT: has_headers(false) because we’re writing records manually
    let mut wtr = csv::WriterBuilder::new()
        .has_headers(false)
        .from_writer(&mut file);

    if write_header {
        wtr.write_record(headers)?;
    }

    for row in rows {
        let rec: Vec<String> = headers
            .iter()
            .map(|h| row.get(h).cloned().unwrap_or_default())
            .collect();
        wtr.write_record(&rec)?;
    }

    wtr.flush()?;
    // file is dropped here and flushed to disk
    Ok(())
}

const MY_METRICS_COLLECTION_PERIOD: Duration = Duration::from_secs(5 * 60);

#[tokio::main]
async fn main() -> Result<()> {
    init_log!();

    // 1) Load symbols (fail loud if bad)
    let symbol_rows = load_symbols_from_csv("indexes/symbols.csv").await?;
    let symbols: Vec<String> = symbol_rows.iter().map(|r| r.symbol.clone()).collect();

    // 2) Configure market data
    let config = MarketDataConfig::builder()
        .subscriptions(
            symbols
                .iter()
                .map(|s| Subscription::new(Symbol::from(s.as_str()), Symbol::from("Binance")))
                .collect::<Vec<_>>(),
        )
        .with_book_manager(true)
        .with_price_tracker(true)
        .build()
        .expect("Failed to build market data");

    config.start()?;
    let (market_data_tx, mut market_data_rx) = unbounded_channel::<Arc<MarketDataEvent>>();

    let market_data = config.expect_market_data_cloned();
    let book_manager = config.expect_book_manager_cloned();
    let price_tracker = config.expect_price_tracker_cloned();

    // Forward market data into the channel
    market_data
        .write()
        .get_multi_observer_arc()
        .write()
        .add_observer_fn({
            let bm = Arc::downgrade(&book_manager);
            let pt = Arc::downgrade(&price_tracker);
            move |event: &Arc<MarketDataEvent>| {
                if let Some(x) = bm.upgrade() {
                    x.write().handle_market_data(event);
                }
                if let Some(y) = pt.upgrade() {
                    y.write().handle_market_data(event);
                }
            }
        });
    let symbols_only: Vec<Symbol> = symbol_rows
        .iter()
        .map(|r| Symbol::from(r.symbol.as_str()))
        .collect();

    wait_for_data(price_tracker.clone(), book_manager.clone(), &symbols_only).await;
    // Build lookup map for base assets (fail loud on duplicates if that’s a concern)
    let mut base_by_symbol: HashMap<Symbol, String> = HashMap::new();
    for r in &symbol_rows {
        base_by_symbol.insert(Symbol::from(r.symbol.as_str()), r.base_asset.clone());
    }

    // Prepare headers once
    let headers = make_headers();

    // Timer + signals
    let mut check_period = tokio::time::interval(MY_METRICS_COLLECTION_PERIOD);
    // first tick happens after the full period; if you want immediate first run, call check_period.tick().await before loop.
    let mut sigint = signal(SignalKind::interrupt())?;
    let mut sigterm = signal(SignalKind::terminate())?;
    let mut sigquit = signal(SignalKind::quit())?;

    tracing::info!("service started; entering main loop");
    let run_for = Duration::from_secs(24 * 60 * 60);
    let stop_at = tokio::time::Instant::now() + run_for;
    let mut stop_timer = tokio::time::sleep_until(stop_at);
    tokio::pin!(stop_timer);

    let mut ctrlc = tokio::signal::ctrl_c();
    tokio::pin!(ctrlc);
    loop {
        tokio::select! {
            _ = check_period.tick() => {
                if let Err(e) = do_metrics_collection(
                    &symbol_rows,
                    &price_tracker,
                    &book_manager,
                    &base_by_symbol,
                    &headers
                ).await {
                    // Loud but non-fatal: keep service alive unless you want to bail out
                    tracing::warn!("metrics collection failed: {:?}", e);
                }
            }
            _ = &mut stop_timer => {
                tracing::info!("24 hours elapsed; shutting down gracefully");
                break;
            }
            Some(event) = market_data_rx.recv() => {
                // fan-out to trackers/managers
                price_tracker.write().handle_market_data(&event);
                book_manager.write().handle_market_data(&event);
            }

            _ = sigint.recv() => {
                tracing::info!("SIGINT received; shutting down");
                break;
            }
            _ = sigterm.recv() => {
                tracing::info!("SIGTERM received; shutting down");
                break;
            }
            _ = sigquit.recv() => {
                tracing::info!("SIGQUIT received; shutting down");
                break;
            }
        }
    }

    Ok(())
}

/// Wait until *all* symbols have:
///   - a positive VolumeWeighted price, and
///   - an order book with both best bid and best ask present.
/// Returns true if coverage achieved before timeout, else false.
async fn wait_for_data(
    pt: Arc<RwLock<PriceTracker>>,
    bm: Arc<RwLock<PricePointBookManager>>,
    symbols: &[Symbol],
) {
    for _ in 0..60 {
        let have_prices = !pt
            .read()
            .get_prices(PriceType::VolumeWeighted, symbols)
            .prices
            .is_empty();
        let have_books = {
            let g = bm.read();
            symbols.iter().any(|s| g.get_order_book(s).is_some())
        };
        tracing::info!("Warm-up: have {} / {} prices", have_prices, symbols.len());
        if have_prices && have_books {
            break;
        }
        sleep(Duration::from_millis(500)).await;
    }
}
// Runs one 5‑minute batch and appends rows to the hourly CSV.
// Errors bubble up (no silent zeros, no unwrap_or_default).
async fn do_metrics_collection(
    symbol_rows: &[SymbolRow],
    price_tracker: &Arc<RwLock<PriceTracker>>,
    book_manager: &Arc<RwLock<PricePointBookManager>>,
    base_by_symbol: &HashMap<Symbol, String>,
    headers: &[String],
) -> eyre::Result<()> {
    // one 5‑min batch (internally does 60 x 5s snapshots)
    let rows_out = run_5min_batch_usd_stats(
        symbol_rows,
        price_tracker.clone(),
        book_manager.clone(),
        base_by_symbol,
    )
    .await?;

    // ensure dir and append to hourly file
    tokio::fs::create_dir_all("hourly_batches").await?;
    let now = Utc::now();
    let filename = format!(
        "hourly_batches/{}_hour{:02}.csv",
        now.date_naive(),
        now.hour()
    );

    write_csv_append(&filename, headers, &rows_out)?;
    tracing::info!("wrote {} rows to {}", rows_out.len(), filename);
    Ok(())
}
