use std::env;

use binance_spot_connector_rust::{
    http::Credentials,
    hyper::{BinanceHttpClient, Error},
    trade,
};

#[tokio::main]
async fn main() -> Result<(), Error> {
    let api_key = env::var("MY_BINANCE_API_KEY").expect("No API key in env");
    let api_secret = env::var("MY_BINANCE_API_SECRET").expect("No API secret in env");
    let credentials = Credentials::from_hmac(api_key, api_secret);
    let client = BinanceHttpClient::default().credentials(credentials);
    let request = trade::all_orders("BNBUSDT").limit(500);
    let data = client.send(request).await?.into_body_str().await?;
    println!("{}", data);
    Ok(())
}
