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
use tokio::time::sleep;
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
    quote_asset: String,
}

fn stat_suffixes() -> [&'static str; 10] {
    [
        "Samples", "Mean", "Std", "Min", "P05", "P25", "P50", "P75", "P95", "Max",
    ]
}

fn to_bps(d: Amount) -> Amount {
    safe!(d * amount!(10000)).unwrap_or(Amount::ZERO).round()
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
            quote_asset: r[2].to_string(),
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
    use eyre::{eyre, WrapErr};
    use rust_decimal_macros::dec;
    use symm_core::market_data::order_book::order_book_manager::OrderBookManager;

    let pt = price_tracker.read();
    let bm = obm.read();

    // Precompute label universe (Bid/Ask × buckets)
    let labels_bps = range_edges_bps(); // Vec<(i32, i32)>

    // Prepare USD conversion prices: prefer VolumeWeighted; fallback to OBM midpoint; else error.
    let mut usd_px = pt.get_prices(PriceType::VolumeWeighted, symbols).prices;

    // Fallback: midpoint from OBM when tracker missing
    if usd_px.len() < symbols.len() {
        for s in symbols {
            if !usd_px.contains_key(s) {
                if let Some(book) = bm.get_order_book(s) {
                    let bb = book.get_entries(Side::Buy, 1).first().map(|e| e.price);
                    let ba = book.get_entries(Side::Sell, 1).first().map(|e| e.price);
                    if let (Some(b), Some(a)) = (bb, ba) {
                        tracing::warn!(target:"liquidity_tracker", symbol=%s, "Missing VolumeWeighted price – using midpoint fallback");
                        usd_px.insert(
                            s.clone(),
                            safe!(b + a / dec!(2)).ok_or_else(|| {
                                eyre::eyre!("safe!() failed computing <what> for {}", s)
                            })?,
                        );
                    }
                }
            }
        }
    }

    // Hard fail if any symbol still lacks a conversion price
    for s in symbols {
        if !usd_px.contains_key(s) {
            return Err(eyre!(
                "Missing USD conversion price for {s} (tracker+OBM midpoint unavailable)"
            ));
        }
        // sanity
        if usd_px[s] <= Amount::ZERO {
            return Err(eyre!(
                "Non‑positive USD conversion price for {s}: {}",
                usd_px[s]
            ));
        }
    }

    // Helper: base price side (BestAsk for Buy, BestBid for Sell), fallback to OBM top
    let mut base_prices_for_side = |side: Side| -> eyre::Result<HashMap<Symbol, Amount>> {
        let price_type = match side {
            Side::Buy => PriceType::BestAsk,
            Side::Sell => PriceType::BestBid,
        };
        let mut px = pt.get_prices(price_type, symbols).prices;

        for s in symbols {
            if !px.contains_key(s) {
                if let Some(book) = bm.get_order_book(s) {
                    match side {
                        Side::Buy => {
                            if let Some(ask) = book.get_entries(Side::Sell, 1).first() {
                                tracing::warn!(target:"liquidity_tracker", symbol=%s, "Missing {:?} from tracker – using OBM top", price_type);
                                px.insert(s.clone(), ask.price);
                            }
                        }
                        Side::Sell => {
                            if let Some(bid) = book.get_entries(Side::Buy, 1).first() {
                                tracing::warn!(target:"liquidity_tracker", symbol=%s, "Missing {:?} from tracker – using OBM top", price_type);
                                px.insert(s.clone(), bid.price);
                            }
                        }
                    }
                }
            }
        }

        // Hard fail if still missing
        for s in symbols {
            if !px.contains_key(s) {
                return Err(eyre!("Missing base price ({:?}) for {s}", price_type));
            }
            if px[s] <= Amount::ZERO {
                return Err(eyre!(
                    "Non‑positive base price ({:?}) for {s}: {}",
                    price_type,
                    px[s]
                ));
            }
        }
        Ok(px)
    };

    // Initialize output with explicit zeros for EVERY key, so downstream can `expect()`.
    let mut out: HashMap<Symbol, HashMap<String, Amount>> = HashMap::with_capacity(symbols.len());
    for s in symbols {
        let mut cols = HashMap::with_capacity(labels_bps.len() * 2);
        for side_prefix in ["Bid", "Ask"] {
            for (lo, hi) in &labels_bps {
                let key = format!("{side_prefix}({lo}/{hi})");
                cols.insert(key, Amount::ZERO);
            }
        }
        out.insert(s.clone(), cols);
    }

    tracing::info!(target:"liquidity_tracker",
        "snapshot: usd_px={} / {} (conversion price coverage)",
        usd_px.len(), symbols.len()
    );

    for &side in &[Side::Buy, Side::Sell] {
        let base = base_prices_for_side(side)?;
        let opp = side.opposite_side();

        tracing::info!(target:"liquidity_tracker",
            "snapshot: base_prices(side={:?})={} / {}",
            side, base.len(), symbols.len()
        );

        // cum[i][sym] = qty up to threshold i
        let mut cum: Vec<HashMap<Symbol, Amount>> = Vec::with_capacity(BUCKETS.len());
        for &thr in BUCKETS.iter() {
            let factor: Decimal = match side {
                Side::Buy => safe!(Amount::ONE + thr).ok_or_else(|| {
                    eyre::eyre!("safe!(1 + thr) failed for side=Buy, thr={}", thr)
                })?,
                Side::Sell => safe!(Amount::ONE - thr).ok_or_else(|| {
                    eyre::eyre!("safe!(1 - thr) failed for side=Sell, thr={}", thr)
                })?,
            };

            // Limits for all symbols; guaranteed present
            let mut limits = HashMap::with_capacity(base.len());
            for (s, p0) in base.iter() {
                // (*p0) * factor -> wrap in safe! and unwrap to Decimal
                let v: Decimal = safe!((*p0) * factor)
                    .ok_or_else(|| eyre::eyre!("safe!(p0 * factor) failed for {}", s))?;
                limits.insert(s.clone(), v);
            }

            let qty_map = OrderBookManager::get_liquidity(&*bm, opp, &limits)
                .wrap_err_with(|| eyre!("get_liquidity failed for side={:?}", side))?;

            // Ensure the engine returned ALL requested symbols
            for s in symbols {
                if !qty_map.contains_key(s) {
                    return Err(eyre!(
                        "get_liquidity missing symbol {} for side={:?} thr={}",
                        s,
                        side,
                        thr
                    ));
                }
                if qty_map[s] < Amount::ZERO {
                    return Err(eyre!(
                        "Negative liquidity for {} side={:?} thr={}: {}",
                        s,
                        side,
                        thr,
                        qty_map[s]
                    ));
                }
            }

            cum.push(qty_map);
        }

        // Diff cumulatives and convert to USD
        for s in symbols {
            let conv_px = usd_px[s];

            for (i_lo, i_hi) in (0..BUCKETS.len() - 1).zip(1..BUCKETS.len()) {
                let lo_thr = BUCKETS[i_lo];
                let hi_thr = BUCKETS[i_hi];
                let q_lo = *cum[i_lo].get(s).expect("qty_map must contain symbol");
                let q_hi = *cum[i_hi].get(s).expect("qty_map must contain symbol");

                // q_hi >= q_lo guaranteed by engine? If not, enforce/order check:
                if q_hi < q_lo {
                    return Err(eyre!(
                        "Non‑monotonic cumulative liquidity for {} side={:?} ({:?}->{:?})",
                        s,
                        side,
                        lo_thr,
                        hi_thr
                    ));
                }

                let range_qty = safe!((q_hi - q_lo).max(Amount::ZERO));
                if range_qty > Amount::ZERO {
                    // unwrap the product
                    let usd: Decimal = safe!(range_qty * conv_px)
                        .ok_or_else(|| eyre!("safe!(range_qty * conv_px) failed for {s}"))?;

                    let lo_bps = to_bps(lo_thr);
                    let hi_bps = to_bps(hi_thr);
                    let side_tag = match side {
                        Side::Buy => "Bid",
                        Side::Sell => "Ask",
                    };
                    let col = format!("{side_tag}({lo_bps}/{hi_bps})");

                    let entry = out.get_mut(s).expect("pre‑initialized symbol map");
                    let slot = entry.get_mut(&col).expect("pre‑initialized key");

                    // unwrap the sum
                    *slot = safe!(*slot + usd)
                        .ok_or_else(|| eyre!("safe!(*slot + usd) overflow for {s} {col}"))?;
                }
                // else: true zero stays explicitly as 0
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
    use eyre::eyre;
    use tokio::time::{sleep, Duration};

    let total_snaps = 60usize; // 5 minutes @ 5s
    let symbols_only: Vec<Symbol> = symbols_rows
        .iter()
        .map(|r| Symbol::from(r.symbol.as_str()))
        .collect();

    let ts_start = chrono::Utc::now();

    // Pre-initialize: symbol -> key -> Vec<Decimal> with capacity = total_snaps
    let labels_bps = range_edges_bps(); // Vec<(lo_bps, hi_bps)>
    let mut samples: HashMap<Symbol, HashMap<String, Vec<Decimal>>> =
        HashMap::with_capacity(symbols_only.len());
    for sym in &symbols_only {
        let mut cols: HashMap<String, Vec<Decimal>> = HashMap::with_capacity(labels_bps.len() * 2);
        for side_prefix in ["Bid", "Ask"] {
            for (lo, hi) in &labels_bps {
                let key = format!("{side_prefix}({lo}/{hi})");
                cols.insert(key, Vec::with_capacity(total_snaps));
            }
        }
        samples.insert(sym.clone(), cols);
    }

    // Collect 60 snapshots
    for i in 0..total_snaps {
        tracing::info!(target:"liquidity_tracker","Snapshot {}/{}", i + 1, total_snaps);
        let snap = take_liquidity_snapshot_usd(&symbols_only, &price_tracker, &obm, None)?;

        // Strong guarantees: per symbol we must have all keys present
        for sym in &symbols_only {
            let cols = snap
                .get(sym)
                .ok_or_else(|| eyre!(format!("Snapshot missing symbol {}", sym)))?;

            let entry = samples
                .get_mut(sym)
                .expect("pre-initialized samples must contain symbol");

            for side_prefix in ["Bid", "Ask"] {
                for (lo, hi) in &labels_bps {
                    let key = format!("{side_prefix}({lo}/{hi})");
                    let v = *cols
                        .get(&key)
                        .expect("take_liquidity_snapshot_usd pre-fills all keys");
                    entry
                        .get_mut(&key)
                        .expect("pre-initialized key must exist")
                        .push(v);
                }
            }
        }

        sleep(Duration::from_secs(5)).await;
    }

    let ts_end = chrono::Utc::now();

    // Build rows with stats per symbol
    let mut rows: Vec<HashMap<String, String>> = Vec::with_capacity(symbols_only.len());

    for sym in &symbols_only {
        // Strict: base asset must be present
        let base_asset = base_by_symbol
            .get(sym)
            .ok_or_else(|| eyre!(format!("Missing base asset for {}", sym)))?
            .clone();

        let per_col = samples
            .remove(sym)
            .expect("pre-initialized samples must contain symbol");

        let mut row = HashMap::new();
        row.insert("TimestampStart".into(), ts_start.timestamp().to_string());
        row.insert("TimestampEnd".into(), ts_end.timestamp().to_string());
        row.insert("Symbol".into(), sym.to_string());
        row.insert("Base Asset".into(), base_asset);

        // We emit Bid first, then Ask (as requested)
        for side_prefix in ["Bid", "Ask"] {
            for (lo, hi) in &labels_bps {
                let base_key = format!("{side_prefix}({lo}/{hi})");
                let vec_ref = per_col
                    .get(&base_key)
                    .ok_or_else(|| eyre!(format!("Missing vector for {}", base_key)))?;
                if vec_ref.len() != total_snaps {
                    return Err(eyre!(
                        "Unexpected sample count for {} {}: got {}, expected {}",
                        sym,
                        base_key,
                        vec_ref.len(),
                        total_snaps
                    ));
                }

                // compute_stats takes ownership; clone the small Vec<Decimal>
                let st = compute_stats(vec_ref.clone(), total_snaps);

                // Suffixes: no space => Bid(50/70)Min, Ask(10/20)Mean, etc.
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

    let (market_data_tx, mut market_data_rx) = unbounded_channel::<Arc<MarketDataEvent>>();

    let market_data = config.expect_market_data_cloned();
    let price_tracker = config.expect_price_tracker_cloned();
    let book_manager = config.expect_book_manager_cloned();

    // Forward market data into the channel
    market_data
        .write()
        .get_multi_observer_arc()
        .write()
        .add_observer_fn(move |event: &Arc<MarketDataEvent>| {
            if let Err(err) = market_data_tx.send(event.clone()) {
                tracing::warn!("Failed to send market data: {:?}", err);
            }
        });

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

    // Start market data
    config.start()?; // or market_data.start() if your API uses that; keep it consistent with your connector

    tracing::info!("service started; entering main loop");
    sleep(Duration::from_secs(60)).await;
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
