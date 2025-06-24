use std::{
    env,
    sync::{Arc, RwLock as ComponentLock},
};

use binance_order_sending::credentials::Credentials;
use chrono::{DateTime, TimeDelta, Utc};
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
        simple_solver::SimpleSolverConfig,
        solver::SolverConfig,
    },
    blockchain::chain_connector::ChainConnector,
    collateral::collateral_router::test_util,
    index::basket::Basket,
    server::server::{Server, ServerResponse},
    solver::solver::OrderIdProvider,
};
use parking_lot::RwLock;
use rust_decimal::dec;
use symm_core::{
    core::{
        bits::{Address, Amount, BatchOrderId, OrderId, PaymentId, Side, Symbol},
        logging::log_init,
        test_util::get_mock_defer_channel,
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

pub struct TimestampOrderIds {}

impl OrderIdProvider for TimestampOrderIds {
    fn next_order_id(&mut self) -> OrderId {
        OrderId::from(format!("O-{}", Utc::now().timestamp_millis()))
    }

    fn next_batch_order_id(&mut self) -> BatchOrderId {
        BatchOrderId::from(format!("B-{}", Utc::now().timestamp_millis()))
    }

    fn next_payment_id(&mut self) -> PaymentId {
        PaymentId::from(format!("P-{}", Utc::now().timestamp_millis()))
    }
}

struct SimpleServer {}

impl Server for SimpleServer {
    fn respond_with(&mut self, response: ServerResponse) {
        tracing::info!("Received response: {:?}", response);
    }
}

struct SimpleChainConnector {}

impl ChainConnector for SimpleChainConnector {
    fn solver_weights_set(&self, symbol: Symbol, basket: Arc<Basket>) {
        tracing::info!("SolverWeightsSet: {}", symbol);
    }

    fn mint_index(
        &self,
        chain_id: u32,
        symbol: Symbol,
        quantity: Amount,
        receipient: Address,
        execution_price: Amount,
        execution_time: DateTime<Utc>,
    ) {
        tracing::info!(
            "MintIndex: {} {:0.5} {:0.5} {}",
            symbol,
            quantity,
            execution_price,
            execution_time
        )
    }

    fn burn_index(
        &self,
        chain_id: u32,
        symbol: Symbol,
        quantity: Amount,
        receipient: symm_core::core::bits::Address,
    ) {
        todo!()
    }

    fn withdraw(
        &self,
        chain_id: u32,
        receipient: symm_core::core::bits::Address,
        amount: Amount,
        execution_price: Amount,
        execution_time: chrono::DateTime<chrono::Utc>,
    ) {
        todo!()
    }
}

#[tokio::main]
async fn main() {
    init_log!();

    let cli = Cli::parse();

    tracing::info!(
        "Index Order: {} {:?} {}",
        cli.symbol,
        cli.side,
        cli.collateral_amount
    );

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

    let symbols = [
        Symbol::from("BNBEUR"),
        Symbol::from("BTCEUR"),
        Symbol::from("ETHEUR"),
        Symbol::from("LINKEUR"),
    ];

    let weights = [dec!(0.3), dec!(0.2), dec!(0.4), dec!(0.1)];
    let index_symbol = Symbol::from("SO4");

    // TODO: This is fake router
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

    // Fake FIX server
    let server = Arc::new(RwLock::new(SimpleServer {}));

    // Fake Blockchain connector
    let chain = Arc::new(ComponentLock::new(SimpleChainConnector {}));

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
        .with_server(server as Arc<RwLock<dyn Server>>)
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

    let order_ids = Arc::new(RwLock::new(TimestampOrderIds {}));

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
        .with_chain_connector(chain as Arc<ComponentLock<dyn ChainConnector + Send + Sync>>)
        .build()
        .expect("Failed to build solver");

    solver_config.run().await.expect("Failed to run solver");

    sleep(std::time::Duration::from_secs(5)).await;

    solver_config.stop().await.expect("Failed to stop solver");
}
