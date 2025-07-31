use std::{sync::Arc, time::Duration};

use binance_market_data::binance_subscriber::{
    BinanceOnlySubscriberTasks, BinanceSubscriberTaskConfig,
};
use market_data::market_data::RealMarketData;
use symm_core::{
    core::{bits::Symbol, functional::IntoObservableManyArc, logging::log_init},
    init_log,
    market_data::market_data_connector::{MarketDataConnector, MarketDataEvent, Subscription},
};
use tokio::time::sleep;

#[tokio::main]
async fn main() {
    init_log!();

    let binance_subscriber_config = BinanceSubscriberTaskConfig {
        subscription_limit_rate: 3,
        stale_check_period: std::time::Duration::from_secs(10),
        stale_timeout: chrono::Duration::seconds(60),
    };
    let mut market_data = RealMarketData::new(
        2,
        std::time::Duration::from_secs(20),
        Arc::new(BinanceOnlySubscriberTasks::new(binance_subscriber_config)),
    );

    market_data
        .get_multi_observer_arc()
        .write()
        .add_observer_fn(move |e: &Arc<MarketDataEvent>| {
            match &**e {
                MarketDataEvent::Trade {
                    symbol,
                    sequence_number,
                    price,
                    quantity,
                } => {
                    tracing::info!("Got trade for {} seq {}", symbol, sequence_number);
                }
                MarketDataEvent::TopOfBook {
                    symbol,
                    sequence_number,
                    best_bid_price,
                    best_ask_price,
                    best_bid_quantity,
                    best_ask_quantity,
                } => {
                    tracing::debug!("Got TOB for {} seq {}", symbol, sequence_number);
                }
                MarketDataEvent::OrderBookSnapshot {
                    symbol,
                    sequence_number,
                    bid_updates,
                    ask_updates,
                } => {
                    tracing::info!("Got snapshot for {} seq {}", symbol, sequence_number);
                }
                MarketDataEvent::OrderBookDelta {
                    symbol,
                    sequence_number,
                    bid_updates,
                    ask_updates,
                } => {
                    tracing::info!("Got delta for {} seq {}", symbol, sequence_number);
                }
            };
        });

    market_data.start().expect("Failed to start market data");
    market_data
        .subscribe(&[Subscription::new(
            Symbol::from("BNBUSDT"),
            Symbol::from("Binance"),
        )])
        .expect("Failed to subscribe");

    sleep(Duration::from_secs(5)).await;

    tracing::info!("Second stage. Subscribing to another pair.");

    market_data
        .subscribe(&[Subscription::new(
            Symbol::from("BTCUSDT"),
            Symbol::from("Binance"),
        )])
        .expect("Failed to subscribe");

    sleep(Duration::from_secs(10)).await;

    market_data
        .stop()
        .await
        .expect("Failed to stop market data");
}
