use std::sync::Arc;

use chrono::Duration;
use index_maker::app::market_data::MarketDataConfig;
use itertools::Itertools;
use rust_decimal::dec;
use symm_core::{
    core::{
        async_loop::AsyncLoop, bits::Symbol, functional::IntoObservableManyArc, logging::log_init,
    },
    init_log,
    market_data::market_data_connector::{MarketDataEvent, Subscription},
};
use tokio::{
    signal::unix::{signal, SignalKind},
    sync::mpsc::unbounded_channel,
};
use tracker::{metrics_collector::MetricsCollector, symbols_csv::Assets};

#[tokio::main]
async fn main() {
    init_log!();

    let assets = Assets::try_new_from_csv("indexes/symbols.csv")
        .await
        .expect("Failed to load assets");

    let market_data_config = MarketDataConfig::builder()
        .subscriptions(
            assets
                .assets
                .iter()
                .map(|asset| {
                    Subscription::new(asset.traded_market.clone(), Symbol::from("Binance"))
                })
                .collect_vec(),
        )
        .with_book_manager(true)
        .with_price_tracker(true)
        .build()
        .expect("Failed to build market data");

    let market_data = market_data_config.expect_market_data_cloned();
    let price_tracker = market_data_config.expect_price_tracker_cloned();
    let book_manager = market_data_config.expect_book_manager_cloned();

    let price_tracker_clone = price_tracker.clone();
    let book_manager_clone = book_manager.clone();

    let (market_data_tx, mut market_data_rx) = unbounded_channel();

    market_data
        .write()
        .get_multi_observer_arc()
        .write()
        .add_observer_fn(move |event: &Arc<MarketDataEvent>| {
            if let Err(err) = market_data_tx.send(event.clone()) {
                // will hapen when channel gets closed when app is terminated
                tracing::trace!("Failed to send market data event for processing: {:?}", err);
            }
        });

    let mut market_data_task = AsyncLoop::new();
    let mut processing_task = AsyncLoop::new();

    market_data_task.start(async move |cancel_token| {
        tracing::info!("Market data processing started");
        loop {
            tokio::select! {
                _ = cancel_token.cancelled() => {
                    break;
                },
                Some(event) = market_data_rx.recv() => {
                    price_tracker_clone.write().handle_market_data(&event);
                    book_manager_clone.write().handle_market_data(&event);
                }
            }
        }
        tracing::info!("Market data processing finished");
    });

    let symbols = assets
        .assets
        .iter()
        .map(|asset| asset.traded_market.clone())
        .collect_vec();

    let mut tick_period = tokio::time::interval(std::time::Duration::from_secs(5));
    let mut metrics_collector = MetricsCollector::new(
        price_tracker,
        book_manager,
        symbols,
        vec![
            "10", "20", "30", "40", "50", "75", "100", "150", "200", "300", "400", "500",
        ],
        vec![
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
        ],
        Duration::minutes(5),
    );

    processing_task.start(async move |cancel_token| {
        tracing::info!("Processing started");
        loop {
            tokio::select! {
                _ = cancel_token.cancelled() => {
                    break;
                },
                _ = tick_period.tick() => {
                    if let Err(err) = metrics_collector.collect_metrics() {
                        tracing::warn!("Failed to collect metrics: {:?}", err);
                    }
                }
            }
        }
        tracing::info!("Processing finished");
    });

    market_data_config
        .start()
        .expect("Failed to start market data");

    let run_for = std::time::Duration::from_secs(24 * 60 * 60);
    let stop_at = tokio::time::Instant::now() + run_for;
    let stop_timer = tokio::time::sleep_until(stop_at);
    tokio::pin!(stop_timer);

    let mut sigint = signal(SignalKind::interrupt()).expect("Failed to attach interrupt handler");
    let mut sigterm = signal(SignalKind::terminate()).expect("Failed to attach terminate handler");
    let mut sigquit = signal(SignalKind::quit()).expect("Failed to attach quit handler");

    loop {
        tokio::select! {
            _ = &mut stop_timer => {
                tracing::info!("24 hours elapsed; shutting down gracefully");
                break;
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

    market_data_task
        .stop()
        .await
        .expect("Failed to stop market data processing");

    processing_task
        .stop()
        .await
        .expect("Failed to stop processing");
}
