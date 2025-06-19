use std::env;

#[tokio::main]
async fn main() {
    let api_key = env::var("MY_BINANCE_API_KEY").expect("No API key in env");
    let api_secret = env::var("MY_BINANCE_API_SECRET").expect("No API secret in env");

    use binance_sdk::config::ConfigurationRestApi;
    use binance_sdk::spot;

    let configuration = ConfigurationRestApi::builder()
        .api_key(api_key)
        .api_secret(api_secret)
        .build()
        .unwrap();

    let client = spot::SpotRestApi::production(configuration);
    let params = spot::rest_api::AllOrdersParams::builder("BNBUSDT".to_string())
        .build()
        .unwrap();
    let response = client.all_orders(params).await.unwrap();

    let data = response.data().await.unwrap();
    println!("{:#?}", data);
}
