use std::time::Duration;

use binance_market_data::binance_market_data::BinanceMarketData;
use index_maker::market_data::market_data_connector::MarketDataConnector;
use tokio::time::sleep;

#[tokio::main]
async fn main() {
    let mut market_data = BinanceMarketData::new(2);

    market_data.start().expect("Failed to start market data");
    market_data
        .subscribe(&["BNBUSDT".into()])
        .expect("Failed to subscribe");

    sleep(Duration::from_secs(3)).await;
    
    market_data
        .subscribe(&["BTCUSDT".into()])
        .expect("Failed to subscribe");
    
    sleep(Duration::from_secs(10)).await;

    market_data
        .stop()
        .await
        .expect("Failed to stop market data");
}
