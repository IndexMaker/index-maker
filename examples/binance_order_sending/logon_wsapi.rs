use std::env;

use binance_order_sending::config::ConfigureBinanceAccess;
use binance_sdk::config::ConfigurationWebsocketApi;
use binance_sdk::spot;
use symm_core::core::logging::log_init;
use symm_core::init_log;

#[tokio::main]
async fn main() {
    init_log!();

    let api_key = env::var("BINANCE_API_KEY").ok();
    let api_secret = env::var("BINANCE_API_SECRET").ok();
    let private_key_file = env::var("BINANCE_PRIVATE_KEY_FILE").ok();

    let configuration = ConfigurationWebsocketApi::builder()
        .configure(api_key, api_secret, private_key_file, None)
        .expect("Failed to configure Binance access")
        .build()
        .unwrap();

    let client = spot::SpotWsApi::production(configuration);
    let connection = client.connect().await.unwrap();
    let params = spot::websocket_api::SessionLogonParamsBuilder::default()
        .build()
        .unwrap();
    let response = connection.session_logon(params).await.unwrap();

    let data = response.data().unwrap();
    tracing::debug!("{:#?}", data);

    let params = spot::websocket_api::SessionLogoutParamsBuilder::default()
        .build()
        .unwrap();
    let response = connection.session_logout(params).await.unwrap();

    let data = response.data().unwrap();
    tracing::debug!("{:#?}", data);
}
