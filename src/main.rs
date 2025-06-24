use std::{env, sync::{Arc, RwLock as ComponentLock}, thread};

use binance_order_sending::credentials::Credentials;
use chrono::TimeDelta;
use clap::Parser;
use crossbeam::{channel::unbounded, select};
use index_maker::{
    app::{
        basket_manager::BasketManagerConfig,
        batch_manager::{BatchManagerConfig, TimestampOrderIds},
        collateral_manager::CollateralManagerConfig,
        market_data::MarketDataConfig,
        order_sender::OrderSenderConfig,
        simple_solver::SimpleSolverConfig, solver::SolverConfig,
    },
    collateral::collateral_router::test_util,
    solver::{solver::{OrderIdProvider, Solver}, solver_quote::SolverClientQuotes},
};
use parking_lot::RwLock;
use rust_decimal::dec;
use symm_core::{
    core::{
        bits::{Amount, Side, Symbol},
        logging::log_init,
        test_util::get_mock_defer_channel,
    },
    init_log,
};
use tokio::{sync::oneshot, time::sleep};

#[derive(Parser)]
struct Cli {
    symbol: Symbol,
    side: Side,
    collateral_amount: Amount,
}

#[tokio::main]
async fn main() {
    init_log!();

    let cli = Cli::parse();

    let price_threshold = dec!(0.01);
    let fee_factor = dec!(1.001);
    let max_order_volley_size = dec!(20.0);
    let max_volley_size = dec!(100.0);

    let fill_threshold = dec!(0.9999);
    let mint_threshold = dec!(0.99);
    let mint_wait_period = TimeDelta::seconds(10);

    let max_batch_size = 4;
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

    let symbols = [
        Symbol::from("BNBEUR"),
        Symbol::from("BTCEUR"),
        Symbol::from("ETHEUR"),
        Symbol::from("LINKEUR"),
    ];

    let weights = [dec!(0.3), dec!(0.2), dec!(0.4), dec!(0.1)];
    let index_symbol = Symbol::from("SO4");

    let (router_tx, router_rx) = get_mock_defer_channel();
    let router = test_util::build_test_router(
        &router_tx,
        &["SRC:BINANCE:EUR", "DST:BINANCE:EUR"],
        &[("SRC:BINANCE:EUR", "DST:BINANCE:EUR")],
        &[&["SRC:BINANCE:EUR", "DST:BINANCE:EUR"]],
        &[(1, "SRC:BINANCE:EUR")],
        "DST:BINANCE:EUR",
        |_, _| (Amount::ZERO, Amount::ZERO),
    );

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

    let batch_manager_config = BatchManagerConfig::builder()
        .zero_threshold(zero_threshold)
        .fill_threshold(fill_threshold)
        .mint_threshold(dec!(0.99))
        .mint_wait_period(TimeDelta::seconds(10))
        .max_batch_size(4)
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

    let order_ids = Arc::new(RwLock::new(TimestampOrderIds {}));

    let solver_config = SolverConfig::builder()
        .zero_threshold(zero_threshold)
        .max_batch_size(max_batch_size)
        .client_order_wait_period(client_order_wait_period)
        .client_quote_wait_period(client_quote_wait_period)
        .with_basket_manager(basket_manager_config)
        .with_batch_manager(batch_manager_config)
        .with_collateral_manager(collateral_manager_config)
        .with_market_data(market_data_config)
        .with_order_sender(order_sender_config)
        .with_strategy(strategy_config)
        .with_order_ids(order_ids as Arc<RwLock<dyn OrderIdProvider + Send + Sync>>)
        /* todo
        .with_index_order_manager(value)
        .with_quote_request_manager(value)
        .with_chain_connector(value)
         */
        .build();

    let (stop_tx, stop_rx) = unbounded::<()>();
    let (stopped_tx, stopped_rx) = oneshot::channel();

    thread::spawn(move || loop {
        select! {
            recv(stop_rx) -> _ => {
                stopped_tx.send(()).unwrap();
                break;
            },
            recv(connector_event_rx) -> res => {
                order_tracker.write().handle_order_notification(res.unwrap()).unwrap();
            },
            recv(order_tracker_rx) -> res => {
                inventory_manager.write().handle_fill_report(res.unwrap()).unwrap();
            },
            recv(inventory_event_rx) -> res => {
                tracing::warn!("Inventory Event unexpected: {:?}", res.unwrap());
            }
        }
    });

    market_data.start().expect("Failed to start market data");
    order_sender.start().expect("Failed to start order sender");

    sleep(std::time::Duration::from_secs(5)).await;

    order_sender.write().stop().await.unwrap();

    stop_tx.send(()).unwrap();
    stopped_rx.await.unwrap();

    market_data.write().stop().await.unwrap();

    // TODO: replace unwrap() with error logging
}
