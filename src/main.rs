use binance_order_sending::credentials::Credentials;
use chrono::{Duration, TimeDelta, Utc};
use clap::{Parser, Subcommand};
use index_maker::{
    app::{
        basket_manager::BasketManagerConfig,
        batch_manager::BatchManagerConfig,
        collateral_manager::CollateralManagerConfig,
        fix_server::FixServerConfig,
        index_order_manager::IndexOrderManagerConfig,
        market_data::MarketDataConfig,
        order_sender::OrderSenderConfig,
        quote_request_manager::QuoteRequestManagerConfig,
        simple_chain::SimpleChainConnectorConfig,
        simple_router::SimpleCollateralRouterConfig,
        simple_server::SimpleServerConfig,
        simple_solver::SimpleSolverConfig,
        solver::{
            ChainConnectorConfig, OrderIdProviderConfig, ServerConfig, SolverConfig,
            SolverStrategyConfig,
        },
        timestamp_ids::TimestampOrderIdsConfig,
    },
    blockchain::chain_connector::ChainNotification,
    server::server::{Server, ServerEvent},
};
use itertools::Itertools;
use parking_lot::RwLock;
use rust_decimal::dec;
use std::{env, sync::Arc};
use symm_core::{
    core::{
        bits::{Amount, PriceType, Side, Symbol},
        logging::log_init,
        test_util::{get_mock_address_1, get_mock_address_2},
    },
    init_log,
    market_data::market_data_connector::Subscription,
};
use tokio::{
    signal::unix::{signal, SignalKind},
    time::sleep,
};

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,

    #[arg(long, short)]
    main_quote_currency: Option<Symbol>,

    #[arg(long, short)]
    bind_address: Option<String>,

    #[arg(long, short)]
    log_path: Option<String>,

    #[arg(long, short)]
    config_path: Option<String>,

    #[arg(long, short, action = clap::ArgAction::SetTrue)]
    term_log_off: bool,

    #[arg(long)]
    otlp_trace_url: Option<String>,

    #[arg(long)]
    otlp_log_url: Option<String>,
}

#[derive(Subcommand, Debug)]
enum Commands {
    SendOrder {
        side: Side,
        symbol: Symbol,
        collateral_amount: Amount,
    },
    FixServer {
        collateral_amount: Amount,
    },
    QuoteServer {},
}

enum AppMode {
    SendOrder {
        side: Side,
        symbol: Symbol,
        collateral_amount: Amount,
        simple_server_config: Arc<dyn ServerConfig + Send + Sync>,
        simple_server: Arc<RwLock<dyn Server + Send + Sync>>,
    },
    FixServer {
        collateral_amount: Amount,
        fix_server_config: Arc<FixServerConfig>,
    },
    QuoteServer {
        fix_server_config: Arc<FixServerConfig>,
    },
}

impl AppMode {
    fn new(command: &Commands, address: String) -> Self {
        match command {
            Commands::SendOrder {
                side,
                symbol,
                collateral_amount,
            } => {
                let config = SimpleServerConfig::builder()
                    .build_arc()
                    .expect("Failed to build server");
                let server = config.expect_server_cloned();

                AppMode::SendOrder {
                    side: *side,
                    symbol: symbol.clone(),
                    collateral_amount: *collateral_amount,
                    simple_server_config: config,
                    simple_server: server,
                }
            }
            Commands::FixServer { collateral_amount } => {
                let config = FixServerConfig::builder()
                    .address(address)
                    .build_arc()
                    .expect("Failed to build server");

                AppMode::FixServer {
                    collateral_amount: *collateral_amount,
                    fix_server_config: config,
                }
            }
            Commands::QuoteServer {} => {
                let config = FixServerConfig::builder()
                    .address(address)
                    .build_arc()
                    .expect("Failed to build server");

                AppMode::QuoteServer {
                    fix_server_config: config,
                }
            }
        }
    }

    fn get_server_config(&self) -> Arc<dyn ServerConfig + Send + Sync> {
        match self {
            Self::SendOrder {
                simple_server_config,
                ..
            } => simple_server_config.clone(),
            Self::FixServer {
                fix_server_config, ..
            } => fix_server_config.clone(),
            Self::QuoteServer { fix_server_config } => fix_server_config.clone(),
        }
    }

    async fn run(&self) {
        match self {
            Self::SendOrder { .. } => {}
            Self::FixServer {
                fix_server_config, ..
            } => {
                fix_server_config
                    .start()
                    .await
                    .expect("Failed to start FIX Server");
            }
            Self::QuoteServer { fix_server_config } => {
                fix_server_config
                    .start()
                    .await
                    .expect("Failed to start FIX Server");
            }
        }
    }

    async fn stop(&self) {
        match self {
            Self::SendOrder { .. } => {}
            Self::FixServer {
                fix_server_config, ..
            } => {
                fix_server_config
                    .stop()
                    .await
                    .expect("Failed to stop FIX Server");
            }
            Self::QuoteServer { fix_server_config } => {
                fix_server_config
                    .stop()
                    .await
                    .expect("Failed to stop FIX Server");
            }
        }
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // ==== Command line input
    // ----

    let cli = Cli::parse();

    init_log!(
        cli.log_path.clone(),
        cli.term_log_off,
        cli.otlp_trace_url,
        cli.otlp_log_url
    );

    match &cli.command {
        Commands::SendOrder {
            side,
            symbol,
            collateral_amount,
        } => tracing::info!("Index Order: {} {:?} {}", symbol, side, collateral_amount),
        Commands::FixServer { collateral_amount } => {
            tracing::info!("FIX Server: {}", collateral_amount)
        }
        Commands::QuoteServer {} => tracing::info!("Quote FIX Server"),
    }

    let config_path = cli.config_path.unwrap_or("configs".into());
    let main_quote_currency = cli.main_quote_currency.unwrap_or("USDT".into());
    let bind_address = cli.bind_address.unwrap_or(String::from("127.0.0.1:3000"));

    let app_mode = AppMode::new(&cli.command, bind_address);

    // ==== Configuration parameters
    // ----

    let price_threshold = dec!(0.0001);
    let fee_factor = dec!(1.001);
    let max_order_volley_size = dec!(20.0);
    let max_volley_size = dec!(100.0);
    let min_asset_volley_size = dec!(5.0);
    let asset_volley_step_size = dec!(0.2);
    let max_total_volley_size = dec!(1000.0);
    let min_total_volley_available = dec!(100.0);

    let fill_threshold = dec!(0.9999);
    let mint_threshold = dec!(0.99);
    let mint_wait_period = TimeDelta::seconds(10);

    let max_batch_size = 4usize;
    let zero_threshold = dec!(0.00001);
    let client_order_wait_period = TimeDelta::seconds(5);
    let client_quote_wait_period = TimeDelta::seconds(1);

    let trading_enabled = env::var("BINANCE_TRADING_ENABLED")
        .map(|s| {
            1 == s
                .parse::<i32>()
                .expect("Failed to parse BINANCE_TRADING_ENABLED environment variable")
        })
        .unwrap_or_default();

    let credentials = Credentials::new(
        String::from("BinanceAccount-1"),
        trading_enabled,
        move || env::var("BINANCE_API_KEY").ok(),
        move || env::var("BINANCE_API_SECRET").ok(),
        move || env::var("BINANCE_PRIVATE_KEY_FILE").ok(),
        move || env::var("BINANCE_PRIVATE_KEY_PHRASE").ok(),
    );

    // ==== Fake stuff
    // ----

    let router_config = SimpleCollateralRouterConfig::builder()
        .chain_id(1u32)
        .source(format!("SRC:BINANCE:{}", main_quote_currency))
        .destination(format!("DST:BINANCE:{}", main_quote_currency))
        .build()
        .expect("Failed to build collateral router");

    let order_id_config = TimestampOrderIdsConfig::builder()
        .build_arc()
        .expect("Failed to build order ID provider");

    let timestamp_order_ids = order_id_config.expect_timestamp_order_ids_cloned();

    let chain_connector_config = SimpleChainConnectorConfig::builder()
        .build_arc()
        .expect("Failed to build chain connector");

    let simple_chain = chain_connector_config.expect_chain_connector_cloned();

    // ==== Real stuff
    // ----

    tracing::info!("Configuring solver...");
    let basket_manager_config = BasketManagerConfig::builder()
        .with_config_file(format!("{}/BasketManagerConfig.json", config_path))
        .build()
        .expect("Failed to build basket manager");

    let symbols = basket_manager_config.get_symbols();
    //let asset_manager = basket_manager_config.expect_asset_manager_cloned();
    // Here all json of basket and assets are loaded

    let market_data_config = MarketDataConfig::builder()
        .zero_threshold(zero_threshold)
        .subscriptions(
            symbols
                .iter()
                .map(|s| Subscription::new(s.clone(), Symbol::from("Binance")))
                .collect_vec(),
        )
        .with_price_tracker(true)
        .with_book_manager(true)
        .build()
        .expect("Failed to build market data");

    let price_tracker = market_data_config.expect_price_tracker_cloned();

    let order_sender_config = OrderSenderConfig::builder()
        .credentials(vec![credentials])
        .symbols(symbols.clone())
        .build()
        .expect("Failed to build order sender");

    let index_order_manager_config = IndexOrderManagerConfig::builder()
        .zero_threshold(zero_threshold)
        .with_server(app_mode.get_server_config())
        .build()
        .expect("Failed to build index order manager");

    let quote_request_manager_config = QuoteRequestManagerConfig::builder()
        .with_server(app_mode.get_server_config())
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

    let collateral_manager_config = CollateralManagerConfig::builder()
        .zero_threshold(zero_threshold)
        .with_router(router_config)
        .build()
        .expect("Failed to build collateral manager");

    let strategy_config = SimpleSolverConfig::builder()
        .price_threshold(price_threshold)
        .fee_factor(fee_factor)
        .max_order_volley_size(max_order_volley_size)
        .max_volley_size(max_volley_size)
        .min_asset_volley_size(min_asset_volley_size)
        .asset_volley_step_size(asset_volley_step_size)
        .max_total_volley_size(max_total_volley_size)
        .min_total_volley_available(min_total_volley_available)
        .build_arc()
        .expect("Failed to build simple solver");

    let mut solver_config = SolverConfig::builder()
        .zero_threshold(zero_threshold)
        .max_batch_size(max_batch_size)
        .solver_tick_interval(Duration::milliseconds(100))
        .quotes_tick_interval(Duration::milliseconds(10))
        .client_order_wait_period(client_order_wait_period)
        .client_quote_wait_period(client_quote_wait_period)
        .with_basket_manager(basket_manager_config)
        .with_batch_manager(batch_manager_config)
        .with_collateral_manager(collateral_manager_config)
        .with_market_data(market_data_config)
        .with_order_sender(order_sender_config)
        .with_index_order_manager(index_order_manager_config)
        .with_quote_request_manager(quote_request_manager_config)
        .with_strategy(strategy_config as Arc<dyn SolverStrategyConfig + Send + Sync>)
        .with_order_ids(order_id_config as Arc<dyn OrderIdProviderConfig + Send + Sync>)
        .with_chain_connector(chain_connector_config as Arc<dyn ChainConnectorConfig + Send + Sync>)
        .build()
        .expect("Failed to build solver");

    let is_running_quotes = match &cli.command {
        Commands::QuoteServer {} => {
            solver_config
                .run_quotes()
                .await
                .expect("Failed to run quotes solver");
            true
        }
        _ => {
            solver_config.run().await.expect("Failed to run solver");
            false
        }
    };

    app_mode.run().await;

    tracing::info!("Awaiting market data...");
    loop {
        sleep(std::time::Duration::from_secs(1)).await;
        let result = price_tracker
            .read()
            .get_prices(PriceType::BestAsk, &symbols);

        if result.missing_symbols.is_empty() {
            break;
        } else {
            tracing::info!(
                "Awaiting market data for: {}",
                result
                    .missing_symbols
                    .into_iter()
                    .map(|s| format!("{}", s))
                    .join(", ")
            );
        }
    }

    sleep(std::time::Duration::from_secs(2)).await;

    match &app_mode {
        AppMode::SendOrder {
            side,
            symbol,
            collateral_amount,
            simple_server,
            ..
        } => {
            tracing::info!("Sending deposit...");

            simple_chain
                .write()
                .expect("Failed to lock chain connector")
                .publish_event(ChainNotification::Deposit {
                    chain_id: 1,
                    address: get_mock_address_1(),
                    amount: *collateral_amount,
                    timestamp: Utc::now(),
                });

            sleep(std::time::Duration::from_secs(2)).await;

            tracing::info!("Sending index order...");

            simple_server
                .write()
                .publish_event(&Arc::new(ServerEvent::NewIndexOrder {
                    chain_id: 1,
                    address: get_mock_address_1(),
                    client_order_id: timestamp_order_ids.write().make_timestamp_id("C-"),
                    symbol: symbol.clone(),
                    side: *side,
                    collateral_amount: *collateral_amount,
                    timestamp: Utc::now(),
                }));
        }
        AppMode::FixServer {
            collateral_amount,
            fix_server_config,
            ..
        } => {
            tracing::info!("Sending deposit...");

            simple_chain
                .write()
                .expect("Failed to lock chain connector")
                .publish_event(ChainNotification::Deposit {
                    chain_id: 1,
                    address: get_mock_address_1(),
                    amount: *collateral_amount,
                    timestamp: Utc::now(),
                });

            simple_chain
                .write()
                .expect("Failed to lock chain connector")
                .publish_event(ChainNotification::Deposit {
                    chain_id: 1,
                    address: get_mock_address_2(),
                    amount: *collateral_amount,
                    timestamp: Utc::now(),
                });

            tracing::info!("Awaiting index order... (Please, send NewIndexOrder message to FIX server running at: {:?})", fix_server_config.address);
        }
        AppMode::QuoteServer { fix_server_config } => {
            tracing::info!("Awaiting quote request... (Please, send NewQuoteRequest message to FIX server running at: {:?})", fix_server_config.address);
        }
    };

    let mut sigint = signal(SignalKind::interrupt())?;
    let mut sigterm = signal(SignalKind::terminate())?;
    let mut sigquit = signal(SignalKind::quit())?;

    tokio::select! {
        _ = sigint.recv() => {
            tracing::info!("SIGINT received")
        }
        _ = sigterm.recv() => {
            tracing::info!("SIGTERM received")
        }
        _ = sigquit.recv() => {
            tracing::info!("SIGQUIT received")
        }
    }

    tracing::info!("Stopping solver...");

    if is_running_quotes {
        solver_config
            .stop_quotes()
            .await
            .expect("Failed to stop quote solver");
    } else {
        solver_config.stop().await.expect("Failed to stop solver");
    }

    app_mode.stop().await;

    tracing::info!("Done.");

    Ok(())
}
