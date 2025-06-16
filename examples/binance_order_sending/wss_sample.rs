use std::env;

use binance_sdk::config::ConfigurationWebsocketApi;
use binance_sdk::spot;

#[tokio::main]
async fn main() {
    let api_key = env::var("MY_BINANCE_API_KEY").expect("No API key in env");
    let api_secret = env::var("MY_BINANCE_API_SECRET").expect("No API secret in env");
    let configuration = ConfigurationWebsocketApi::builder()
        .api_key(api_key)
        .api_secret(api_secret)
        .build().unwrap();

    let client = spot::SpotWsApi::testnet(configuration);
    let connection = client.connect().await.unwrap();
    let params = spot::websocket_api::ExchangeInfoParams::default();
    let response = connection.exchange_info(params).await.unwrap();

    let data = response.data().unwrap();
    println!("{:#?}", data);
}
