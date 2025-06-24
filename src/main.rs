use std::{env, sync::Arc, thread};

use binance_order_sending::credentials::Credentials;
use chrono::TimeDelta;
use clap::Parser;
use crossbeam::{channel::unbounded, select};
use index_maker::{
    app::{
        basket_manager::BasketManagerConfig, batch_manager::BatchManagerConfig,
        inventory_manager::InventoryManagerConfig, market_data::MarketDataConfig,
        order_ids::TimestampOrderIds, order_sender::OrderSenderConfig,
        order_tracker::OrderTrackerConfig,
    },
    solver::{solver::Solver, solvers::simple_solver::SimpleSolver},
};
use parking_lot::RwLock;
use rust_decimal::dec;
use symm_core::{
    core::{
        bits::{Amount, Side, Symbol},
        functional::IntoObservableSingle,
        logging::log_init,
    },
    init_log,
    market_data::{order_book::order_book_manager::OrderBookEvent, price_tracker::PriceEvent},
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

    let market_data_config = MarketDataConfig::builder()
        .zero_threshold(zero_threshold)
        .symbols(&symbols)
        .try_build()
        .expect("Failed to build market data config");

    let (market_data, price_tracker, book_manager) = market_data_config
        .make()
        .expect("Failed to start market data");

    let order_sender_config = OrderSenderConfig::builder()
        .credentials(vec![credentials])
        .try_build()
        .expect("Failed to build order sender config");

    let (order_sender, connector_event_rx) = order_sender_config
        .make()
        .expect("Failed to start order sender");

    let order_tracker_config = OrderTrackerConfig::builder()
        .zero_threshold(zero_threshold)
        .try_build()
        .expect("Failed to build order tracker config");

    let (order_tracker, order_tracker_rx) = order_tracker_config
        .make(order_sender.clone())
        .expect("Failed to build order tracker");

    let inventory_manager_config = InventoryManagerConfig::builder()
        .zero_threshold(zero_threshold)
        .try_build()
        .expect("Failed to build inventory manager cnofig");

    let (inventory_manager, inventory_event_rx) = inventory_manager_config
        .make(order_tracker.clone())
        .expect("Failed to build inventory manager");

    let basket_manager_config = BasketManagerConfig::builder()
        .try_build()
        .expect("Failed to build basket manager config");

    let (basket_manager, basket_event_rx) = basket_manager_config
        .make()
        .expect("Failed to build basket manager");

    let batch_manager_config = BatchManagerConfig::builder()
        .try_build()
        .expect("Failed to build batch manager config");

    let (batch_manager, batch_event_rx) = batch_manager_config
        .make()
        .expect("Failed to build batch manager");

    price_tracker
        .write()
        .get_single_observer_mut()
        .set_observer_fn(move |e: PriceEvent| match e {
            PriceEvent::PriceChange { symbol } => tracing::trace!("PriceInfo {}", symbol),
        });

    book_manager
        .write()
        .get_single_observer_mut()
        .set_observer_fn(move |e: OrderBookEvent| match e {
            OrderBookEvent::BookUpdate { symbol } => {
                tracing::debug!("BookUpdate {}", symbol)
            }
            OrderBookEvent::UpdateError { symbol, error } => {
                tracing::warn!("BookUpdate {}: Error: {}", symbol, error)
            }
        });

    let strategy = Arc::new(SimpleSolver::new(
        price_threshold,
        fee_factor,
        max_order_volley_size,
        max_volley_size,
    ));

    let order_ids = Arc::new(RwLock::new(TimestampOrderIds {}));

    //let solver = Arc::new(ComponentLock::new(Solver::new(
    //    strategy,
    //    order_ids,
    //    basket_manager,
    //    price_tracker,
    //    book_manager,
    //    chain_connector,
    //    batch_manager,
    //    collateral_manager,
    //    index_order_manager,
    //    quote_request_manager,
    //    inventory_manager,
    //    max_batch_size,
    //    zero_threshold,
    //    client_order_wait_period,
    //    client_quote_wait_period,
    //)));

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

    sleep(std::time::Duration::from_secs(5)).await;

    order_sender.write().stop().await.unwrap();

    stop_tx.send(()).unwrap();
    stopped_rx.await.unwrap();

    market_data.write().stop().await.unwrap();

    // TODO: replace unwrap() with error logging
}
