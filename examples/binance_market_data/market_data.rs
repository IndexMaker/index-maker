use std::{sync::Arc, time::Duration};

use binance_market_data::binance_market_data::BinanceMarketData;
use symm_core::{
    core::{functional::IntoObservableManyArc, logging::log_init},
    init_log,
    market_data::market_data_connector::{MarketDataConnector, MarketDataEvent},
};
use tokio::time::sleep;

#[tokio::main]
async fn main() {
    init_log!();

    let mut market_data = BinanceMarketData::new(2);

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
        .subscribe(&["BNBUSDT".into()])
        .expect("Failed to subscribe");

    sleep(Duration::from_secs(5)).await;

    tracing::info!("Second stage. Subscribing to another pair.");

    market_data
        .subscribe(&["BTCUSDT".into()])
        .expect("Failed to subscribe");

    sleep(Duration::from_secs(10)).await;

    market_data
        .stop()
        .await
        .expect("Failed to stop market data");
}
