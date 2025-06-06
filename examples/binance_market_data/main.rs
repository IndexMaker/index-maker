use std::{sync::Arc, time::Duration};

use binance_market_data::binance_market_data::BinanceMarketData;
use index_maker::{
    core::functional::IntoObservableMany,
    market_data::market_data_connector::{MarketDataConnector, MarketDataEvent},
};
use tokio::time::sleep;

#[tokio::main]
async fn main() {
    let mut market_data = BinanceMarketData::new(2);

    market_data
        .get_multi_observer_mut()
        .add_observer_fn(move |e: &Arc<MarketDataEvent>| {
            match &**e {
                MarketDataEvent::Trade {
                    symbol,
                    price,
                    quantity,
                } => {
                    println!("Got trade for {}", symbol);
                }
                MarketDataEvent::TopOfBook {
                    symbol,
                    best_bid_price,
                    best_ask_price,
                    best_bid_quantity,
                    best_ask_quantity,
                } => {
                    println!("Got TOB for {}", symbol);
                }
                MarketDataEvent::OrderBookSnapshot {
                    symbol,
                    bid_updates,
                    ask_updates,
                } => {
                    println!("Got snapshot for {}", symbol);
                }
                MarketDataEvent::OrderBookDelta {
                    symbol,
                    bid_updates,
                    ask_updates,
                } => {
                    println!("Got delta for {}", symbol);
                }
            };
        });

    market_data.start().expect("Failed to start market data");
    market_data
        .subscribe(&["BNBUSDT".into()])
        .expect("Failed to subscribe");

    sleep(Duration::from_secs(5)).await;

    println!("Second stage. Subscribing to another pair.");

    market_data
        .subscribe(&["BTCUSDT".into()])
        .expect("Failed to subscribe");

    sleep(Duration::from_secs(10)).await;

    market_data
        .stop()
        .await
        .expect("Failed to stop market data");
}
