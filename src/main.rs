use std::{
    env,
    sync::{Arc, RwLock as ComponentLock},
};

use binance_order_sending::credentials::Credentials;
use chrono::{TimeDelta, Utc};
use clap::Parser;
use index_maker::{
    app::{
        basket_manager::BasketManagerConfig,
        batch_manager::BatchManagerConfig,
        collateral_manager::CollateralManagerConfig,
        index_order_manager::IndexOrderManagerConfig,
        market_data::MarketDataConfig,
        order_sender::OrderSenderConfig,
        quote_request_manager::QuoteRequestManagerConfig,
        simple_chain::SimpleChainConnector,
        simple_router::build_collateral_router,
        simple_server::SimpleServer,
        simple_solver::SimpleSolverConfig,
        solver::SolverConfig,
        timestamp_ids::{util::make_timestamp_id, TimestampOrderIds},
    },
    blockchain::chain_connector::{ChainConnector, ChainNotification},
    index::basket::{AssetWeight, BasketDefinition},
    server::server::{Server, ServerEvent},
    solver::solver::OrderIdProvider,
};
use itertools::Itertools;
use parking_lot::RwLock;
use rust_decimal::dec;
use symm_core::{
    assets::asset::Asset,
    core::{
        bits::{Amount, Side, Symbol},
        logging::log_init,
        test_util::get_mock_address_1,
    },
    init_log,
};
use tokio::time::sleep;

#[derive(Parser)]
struct Cli {
    symbol: Symbol,
    side: Side,
    collateral_amount: Amount,
}

#[tokio::main]
async fn main() {
    init_log!();

    // ==== Command line input
    // ----

    let cli = Cli::parse();

    tracing::info!(
        "Index Order: {} {:?} {}",
        cli.symbol,
        cli.side,
        cli.collateral_amount
    );


    // ==== Configuration parameters
    // ----
    
    let price_threshold = dec!(0.01);
    let fee_factor = dec!(1.001);
    let max_order_volley_size = dec!(20.0);
    let max_volley_size = dec!(100.0);

    let fill_threshold = dec!(0.9999);
    let mint_threshold = dec!(0.99);
    let mint_wait_period = TimeDelta::seconds(10);

    let max_batch_size = 4usize;
    let zero_threshold = dec!(0.00001);
    let client_order_wait_period = TimeDelta::seconds(5);
    let client_quote_wait_period = TimeDelta::seconds(1);

    let api_key = env::var("BINANCE_API_KEY").expect("No API key in env");
    let credentials = Credentials::new(
        api_key,
        move || env::var("BINANCE_API_SECRET").ok(),
        move || env::var("BINANCE_PRIVATE_KEY_FILE").ok(),
        move || env::var("BINANCE_PRIVATE_KEY_PHRASE").ok(),
    );


    // ==== Fake stuff
    // ----

    // Fake index assets (btw: these should be assets and not markets)
    let symbols = [
        Symbol::from("BNBEUR"),
        Symbol::from("BTCEUR"),
        Symbol::from("ETHEUR"),
        Symbol::from("LINKEUR"),
    ];

    let weights = [dec!(0.3), dec!(0.2), dec!(0.4), dec!(0.1)];
    let index_symbol = Symbol::from("SO4");

    let assets = symbols
        .iter()
        .map(|s| Arc::new(Asset::new(s.clone())))
        .collect_vec();

    let asset_weights = assets
        .iter()
        .zip(weights)
        .map(|(asset, weight)| AssetWeight::new(asset.clone(), weight))
        .collect_vec();

    // Fake backet definition
    let basket_definition = BasketDefinition::try_new(asset_weights.into_iter())
        .expect("Failed to create basket definition");

    // Fake FIX server
    let server = Arc::new(RwLock::new(SimpleServer::new()));

    // Fake Blockchain connector
    let chain = Arc::new(ComponentLock::new(SimpleChainConnector::new()));

    // Fake router
    let router = build_collateral_router(1, "SRC:BINANCE:EUR", "DST:BINANCE:EUR");

    // Fake order IDs
    let order_ids = Arc::new(RwLock::new(TimestampOrderIds {}));


    // ==== Real stuff
    // ----
    
    let market_data_config = MarketDataConfig::builder()
        .zero_threshold(zero_threshold)
        .symbols(&symbols)
        .with_price_tracker(true)
        .with_book_manager(true)
        .build()
        .expect("Failed to build market data");

    let order_sender_config = OrderSenderConfig::builder()
        .credentials(vec![credentials])
        .build()
        .expect("Failed to build order sender");

    let index_order_manager_config = IndexOrderManagerConfig::builder()
        .zero_threshold(zero_threshold)
        .with_server(server.clone() as Arc<RwLock<dyn Server>>)
        .build()
        .expect("Failed to build index order manager");

    let quote_request_manager_config = QuoteRequestManagerConfig::builder()
        .with_server(server.clone() as Arc<RwLock<dyn Server>>)
        .build()
        .expect("Failed to build quote request manager");

    let batch_manager_config = BatchManagerConfig::builder()
        .zero_threshold(zero_threshold)
        .fill_threshold(fill_threshold)
        .mint_threshold(mint_threshold)
        .mint_wait_period(mint_wait_period)
        .max_batch_size(max_batch_size)
        .build()
        .expect("Failed to build batch manager");

    let basket_manager_config = BasketManagerConfig::builder()
        .build()
        .expect("Failed to build basket manager");

    let collateral_manager_config = CollateralManagerConfig::builder()
        .zero_threshold(zero_threshold)
        .with_router(router)
        .build()
        .expect("Failed tp build collateral manager");

    let strategy_config = SimpleSolverConfig::builder()
        .price_threshold(price_threshold)
        .fee_factor(fee_factor)
        .max_order_volley_size(max_order_volley_size)
        .max_volley_size(max_volley_size)
        .build()
        .expect("Failed to build simple solver");

    let mut solver_config = SolverConfig::builder()
        .zero_threshold(zero_threshold)
        .max_batch_size(max_batch_size)
        .client_order_wait_period(client_order_wait_period)
        .client_quote_wait_period(client_quote_wait_period)
        .with_basket_manager(basket_manager_config)
        .with_batch_manager(batch_manager_config)
        .with_collateral_manager(collateral_manager_config)
        .with_market_data(market_data_config)
        .with_order_sender(order_sender_config)
        .with_index_order_manager(index_order_manager_config)
        .with_quote_request_manager(quote_request_manager_config)
        .with_strategy(strategy_config)
        .with_order_ids(order_ids as Arc<RwLock<dyn OrderIdProvider + Send + Sync>>)
        .with_chain_connector(chain.clone() as Arc<ComponentLock<dyn ChainConnector + Send + Sync>>)
        .build()
        .expect("Failed to build solver");

    solver_config.run().await.expect("Failed to run solver");

    sleep(std::time::Duration::from_secs(5)).await;

    chain
        .write()
        .expect("Failed to lock chain connector")
        .publish_event(ChainNotification::CuratorWeightsSet(
            index_symbol,
            basket_definition,
        ));

    server
        .read()
        .publish_event(&Arc::new(ServerEvent::NewIndexOrder {
            chain_id: 1,
            address: get_mock_address_1(),
            client_order_id: make_timestamp_id("C-"),
            symbol: cli.symbol,
            side: cli.side,
            collateral_amount: cli.collateral_amount,
            timestamp: Utc::now(),
        }));

    solver_config.stop().await.expect("Failed to stop solver");
}
