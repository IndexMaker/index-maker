use chrono::{Timelike, Utc};
use csv::Reader;
use eyre::Result;
use index_maker::app::market_data::MarketDataConfig;
use parking_lot::RwLock;
use rust_decimal::prelude::ToPrimitive;
use std::{collections::HashMap, sync::Arc, time::Duration};
use symm_core::core::bits::{PricePointEntry, Side};
use symm_core::{
    core::{bits::Symbol, logging::log_init},
    init_log,
    market_data::{
        market_data_connector::Subscription, order_book::order_book_manager::PricePointBookManager,
    },
};
use tokio::{fs, time::sleep};

const BUCKETS: &[f64] = &[
    0.001, 0.002, 0.003, 0.004, 0.005, 0.0075, 0.01, 0.015, 0.02, 0.03, 0.04, 0.05,
];

#[derive(Debug)]
struct SymbolRow {
    symbol: String,
}

fn get_bucket_labels() -> Vec<String> {
    BUCKETS
        .iter()
        .enumerate()
        .map(|(i, &pct)| {
            let lower = if i == 0 { 0.0 } else { BUCKETS[i - 1] };
            format!("{:.1}-{:.1}%", lower * 100.0, pct * 100.0)
        })
        .collect()
}

async fn load_symbols_from_csv(path: &str) -> Result<Vec<SymbolRow>> {
    let mut rdr = Reader::from_path(path)?;
    let mut symbols = Vec::new();

    for result in rdr.records() {
        let record = result?;
        symbols.push(SymbolRow {
            symbol: record[0].trim().to_string(),
        });
    }

    Ok(symbols)
}

fn get_bucket_ranges(mid_price: f64) -> Vec<(String, f64, f64)> {
    BUCKETS
        .iter()
        .enumerate()
        .map(|(i, &pct)| {
            let lower = if i == 0 {
                0.0
            } else {
                BUCKETS[i - 1] * mid_price
            };
            let upper = pct * mid_price;
            (
                format!(
                    "{:.2}-{:.2}%",
                    BUCKETS.get(i - 1).unwrap_or(&0.0) * 100.0,
                    pct * 100.0
                ),
                lower,
                upper,
            )
        })
        .collect()
}

async fn sample_order_book(
    symbol: &str,
    book_manager: Arc<RwLock<PricePointBookManager>>,
) -> HashMap<String, f64> {
    let symbol_obj = Symbol::from(symbol);
    let book_guard = book_manager.read();
    let book_opt = book_guard.get_order_book(&symbol_obj);

    let mut buckets: HashMap<String, f64> = HashMap::new();
    if book_opt.is_none() {
        println!("⚠️ No order book found for symbol: {}", symbol);

        return buckets;
    }

    let book = book_opt.unwrap();

    // Get top-of-book entries
    let best_bid = book.get_entries(Side::Buy, 1).first().cloned();
    let best_ask = book.get_entries(Side::Sell, 1).first().cloned();

    let (bid, ask) = match (best_bid, best_ask) {
        (Some(bid), Some(ask)) => (bid.price, ask.price),
        _ => return buckets, // no valid book
    };

    let mid = ((bid + ask) / rust_decimal::Decimal::TWO)
        .to_f64()
        .unwrap_or(0.0);
    if mid == 0.0 {
        return buckets;
    }

    let ranges = get_bucket_ranges(mid);
    for (label, _, _) in &ranges {
        buckets.insert(label.clone(), 0.0);
    }

    for side in [Side::Buy, Side::Sell] {
        let entries = book.get_entries(side, 100); // sample up to 100 levels
        for PricePointEntry { price, quantity } in entries {
            let price_f = price.to_f64().unwrap_or(0.0);
            let quantity_f = quantity.to_f64().unwrap_or(0.0);
            if price_f == 0.0 || quantity_f == 0.0 {
                continue;
            }

            let distance = (price_f - mid).abs();

            for (label, lower, upper) in &ranges {
                if distance >= *lower && distance < *upper {
                    *buckets.entry(label.clone()).or_default() += price_f * quantity_f;
                    break;
                }
            }
        }
    }
    buckets
}

async fn run_5min_batch(
    symbols: &[String],
    book_manager: Arc<RwLock<PricePointBookManager>>,
) -> Vec<HashMap<String, String>> {
    let mut pair_samples: HashMap<String, Vec<HashMap<String, f64>>> = HashMap::new();

    // 60 samples over 5 minutes (every 5 seconds)
    for _ in 0..60 {
        for symbol in symbols {
            let sample = sample_order_book(symbol, book_manager.clone()).await;
            pair_samples.entry(symbol.clone()).or_default().push(sample);
        }
        sleep(Duration::from_secs(5)).await;
    }

    let mut rows = Vec::new();
    let labels = get_bucket_labels();

    for (symbol, samples) in pair_samples {
        let mut bucket_sums: HashMap<String, f64> = HashMap::new();
        for label in &labels {
            bucket_sums.insert(label.clone(), 0.0);
        }

        for sample in samples {
            for label in &labels {
                if let Some(value) = sample.get(label) {
                    *bucket_sums.entry(label.clone()).or_insert(0.0) += *value;
                }
            }
        }

        // Average over 60 samples
        let mut row = HashMap::new();
        row.insert(
            "Timestamp".to_string(),
            Utc::now().format("%Y-%m-%d %H:%M:%S").to_string(),
        );
        row.insert("Symbol".to_string(), symbol.clone());

        let base_asset = symbol
            .trim_end_matches("USDT")
            .trim_end_matches("USDC")
            .to_string();
        row.insert("Base Asset".to_string(), base_asset);

        for label in &labels {
            let avg = bucket_sums.get(label).cloned().unwrap_or(0.0) / 60.0;
            row.insert(label.clone(), format!("{:.2}", avg));
        }

        rows.push(row);
    }

    rows
}

async fn write_csv(filename: &str, rows: &[HashMap<String, String>]) -> Result<()> {
    if rows.is_empty() {
        return Ok(());
    }

    let bucket_labels = get_bucket_labels();
    let mut headers = vec!["Timestamp", "Symbol", "Base Asset"];
    headers.extend(bucket_labels.iter().map(String::as_str));
    let mut wtr = csv::Writer::from_path(filename)?;

    wtr.write_record(&headers)?;
    for row in rows {
        let record: Vec<String> = headers
            .iter()
            .map(|key| row.get(*key).cloned().unwrap_or_else(|| "".to_string()))
            .collect();
        wtr.write_record(&record)?;
    }

    wtr.flush()?;
    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    init_log!();

    let symbol_rows = load_symbols_from_csv("indexes/symbols.csv").await?;
    let symbols: Vec<String> = symbol_rows.into_iter().map(|r| r.symbol).collect();

    let config = MarketDataConfig::builder()
        .with_book_manager(true)
        .with_price_tracker(true)
        .subscriptions(
            symbols
                .iter()
                .map(|s| Subscription::new(Symbol::from(s.as_str()), Symbol::from("Binance")))
                .collect::<Vec<_>>(),
        )
        .build()?;

    config.start()?;

    let book_manager = config.expect_book_manager_cloned();

    for hour in 0..24 {
        let mut all_rows = Vec::new();
        for batch in 0..1 {
            println!("Hour {}/24, Batch {}/12", hour + 1, batch + 1);
            let batch_data = run_5min_batch(&symbols, book_manager.clone()).await;
            all_rows.extend(batch_data);
        }

        let now = Utc::now();
        let filename = format!(
            "hourly_batches/{}_hour{:02}.csv",
            now.date_naive(),
            now.hour()
        );
        fs::create_dir_all("hourly_batches").await?;
        write_csv(&filename, &all_rows).await?;
        println!("✅ Saved: {}", filename);
    }

    Ok(())
}
