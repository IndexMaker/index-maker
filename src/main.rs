use alloy::primitives::address;
use alloy_chain_connector::{
    chain_connector::GasFeeCalculator,
    credentials::{Credentials as AlloyCredentials, SharedSessionData},
};
use alloy_primitives::{map::foldhash::SharedSeed, U256};
use binance_order_sending::{
    binance_order_sending::BinanceFeeCalculator, credentials::Credentials as BinanceCredentials,
};
use chrono::{Duration, TimeDelta, Utc};
use clap::{Parser, Subcommand};
use index_core::blockchain::chain_connector::ChainNotification;
use index_maker::{
    app::{
        basket_manager::BasketManagerConfig,
        batch_manager::BatchManagerConfig,
        chain_connector::RealChainConnectorConfig,
        collateral_manager::CollateralManagerConfig,
        collateral_router::CollateralRouterConfig,
        fix_server::FixServerConfig,
        index_order_manager::IndexOrderManagerConfig,
        market_data::MarketDataConfig,
        mint_invoice_manager::MintInvoiceManagerConfig,
        order_sender::{OrderSenderConfig, OrderSenderCredentials},
        query_service::QueryServiceConfig,
        quote_request_manager::QuoteRequestManagerConfig,
        simple_chain::SimpleChainConnectorConfig,
        simple_router::SimpleCollateralRouterConfig,
        simple_server::{SimpleServer, SimpleServerConfig},
        simple_solver::SimpleSolverConfig,
        solver::{
            ChainConnectorConfig, OrderIdProviderConfig, ServerConfig, SolverConfig,
            SolverStrategyConfig,
        },
        timestamp_ids::TimestampOrderIdsConfig,
    },
    server::server::ServerEvent,
    solver::mint_invoice_manager,
};
use itertools::{Either, Itertools};
use otc_custody::custody_authority::CustodyAuthority;
use parking_lot::RwLock;
use rust_decimal::dec;
use std::{collections::VecDeque, env, process, sync::Arc};

use symm_core::{
    core::{
        bits::{Amount, PriceType, Side, Symbol},
        logging::log_init,
        test_util::get_mock_address_1,
    },
    market_data::market_data_connector::Subscription,
    order_sender::order_connector::SessionId,
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

    #[arg(long)]
    dry_run: bool,

    #[arg(long, short)]
    simulate_sender: bool,

    #[arg(long)]
    simulate_chain: bool,

    #[arg(long, short)]
    bind_address: Option<String>,

    #[arg(long, short)]
    query_bind_address: Option<String>,

    #[arg(short, long, value_delimiter = ',')]
    rpc_urls: Option<Vec<String>>,

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

    #[arg(long)]
    batch_size: Option<usize>,
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
        simple_server_config: Arc<SimpleServerConfig>,
        simple_server: Arc<RwLock<SimpleServer>>,
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

                let server = config.expect_simple_server_cloned();

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

enum ChainMode {
    Simulated {
        chain_config: Arc<SimpleChainConnectorConfig>,
    },
    Real {
        chain_config: Arc<RealChainConnectorConfig>,
    },
}

impl ChainMode {
    async fn new_with_router(
        simulate_chain: bool,
        rpc_urls: Option<Vec<String>>,
        main_quote_currency: Symbol,
        index_symbols: Vec<Symbol>,
        market_data: &MarketDataConfig,
        config_file: String,
        router_config: &CollateralRouterConfig,
    ) -> Self {
        if simulate_chain {
            let simple_chain_connector_config = SimpleChainConnectorConfig::builder()
                .build_arc()
                .expect("Failed to build chain connector");

            SimpleCollateralRouterConfig::builder()
                .chain_id(1u32)
                .source(format!("SRC:BINANCE:{}", main_quote_currency))
                .destination(format!("DST:BINANCE:{}", main_quote_currency))
                .index_symbols(index_symbols)
                .with_router(router_config.clone())
                .build()
                .expect("Failed to build collateral router");

            Self::Simulated {
                chain_config: simple_chain_connector_config,
            }
        } else {
            let chain_id = 8453;
            let rpc_urls = rpc_urls.unwrap_or_else(|| vec![String::from("http://127.0.0.1:8545")]);
            let mut rpc_urls = VecDeque::from(rpc_urls);
            let concurrent_session_count = 2.min(rpc_urls.len());
            let shared_data = Arc::new(SharedSessionData::new(chain_id, rpc_urls.clone()));

            let index_operator_credentials = vec![AlloyCredentials::new(
                format!("{}", chain_id),
                shared_data.clone(),
                Arc::new(|| {
                    env::var("INDEX_MAKER_PRIVATE_KEY")
                        .expect("INDEX_MAKER_PRIVATE_KEY environment variable must be defined")
                }),
            )];

            let index_operator_custody_auth = CustodyAuthority::new(|| {
                env::var("CUSTODY_AUTHORITY_PRIVATE_KEY")
                    .expect("CUSTODY_AUTHORITY_PRIVATE_KEY environment variable must be defined")
            });

            let gas_fee_calculator = GasFeeCalculator::new(
                market_data.expect_price_tracker_exchange_rates_cloned(),
                main_quote_currency.clone(),
            );

            let chain_connector_config = RealChainConnectorConfig::builder()
                .with_router(router_config.clone())
                .with_credentials(index_operator_credentials)
                .with_main_quote_currency(main_quote_currency)
                .with_custody_authority(index_operator_custody_auth)
                .with_config_file(config_file)
                .with_gas_fee_calculator(gas_fee_calculator)
                .build_arc()
                .await
                .expect("Failed to build chain connector config");

            Self::Real {
                chain_config: chain_connector_config,
            }
        }
    }

    fn get_chain_connector_config(&self) -> Arc<dyn ChainConnectorConfig + Send + Sync> {
        match self {
            ChainMode::Simulated { chain_config } => {
                chain_config.clone() as Arc<dyn ChainConnectorConfig + Send + Sync>
            }
            ChainMode::Real { chain_config } => {
                chain_config.clone() as Arc<dyn ChainConnectorConfig + Send + Sync>
            }
        }
    }

    async fn run(&self) -> eyre::Result<()> {
        match self {
            ChainMode::Simulated { .. } => Ok(()),
            ChainMode::Real { chain_config } => chain_config.start().await,
        }
    }

    async fn stop(&self) -> eyre::Result<()> {
        match self {
            ChainMode::Simulated { .. } => Ok(()),
            ChainMode::Real { chain_config } => chain_config.stop().await,
        }
    }
}

fn get_otlp_url(value: Option<String>) -> Option<Option<String>> {
    if let Some(value) = value {
        if value.as_str().eq("default") {
            Some(None)
        } else {
            Some(Some(value))
        }
    } else {
        None
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // ==== Command line input
    // ----

    let cli = Cli::parse();

    log_init(
        format!("{}=info,Binance=off", env!("CARGO_CRATE_NAME")),
        cli.log_path.clone(),
        cli.term_log_off,
        get_otlp_url(cli.otlp_trace_url),
        get_otlp_url(cli.otlp_log_url),
        cli.batch_size,
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
    let main_quote_currency = cli.main_quote_currency.unwrap_or("USDC".into());
    let bind_address = cli.bind_address.unwrap_or(String::from("127.0.0.1:3000"));

    let app_mode = AppMode::new(&cli.command, bind_address);

    // ==== Configuration parameters
    // ----

    let price_threshold = dec!(0.01);
    let max_levels = 5usize;
    let fee_factor = dec!(1.002);
    let max_order_volley_size = dec!(20.0);
    let max_volley_size = dec!(100.0);
    let min_asset_volley_size = dec!(5.0);
    let asset_volley_step_size = dec!(0.1);
    let max_total_volley_size = dec!(1000.0);
    let min_total_volley_available = dec!(100.0);

    let fill_threshold = dec!(0.9999);
    let mint_threshold = dec!(0.99);
    let mint_wait_period = TimeDelta::seconds(10);

    let max_batch_size = 4usize;
    let zero_threshold = dec!(0.000000000000000001);
    let client_order_wait_period = TimeDelta::seconds(10);
    let client_quote_wait_period = TimeDelta::seconds(1);

    let trading_enabled = env::var("BINANCE_TRADING_ENABLED")
        .map(|s| {
            1 == s
                .parse::<i32>()
                .expect("Failed to parse BINANCE_TRADING_ENABLED environment variable")
        })
        .unwrap_or_default();

    let credentials = if cli.simulate_sender {
        tracing::warn!("Using simulated order sender");
        OrderSenderCredentials::Simple(SessionId::from("SimpleSenderSession"))
    } else {
        tracing::info!(
            "Using Binance order sender. Please, set BINANCE_TRADING_ENABLED=1 to enable trading"
        );
        OrderSenderCredentials::Binance(vec![BinanceCredentials::new(
            String::from("BinanceAccount-1"),
            trading_enabled,
            move || env::var("BINANCE_API_KEY").ok(),
            move || env::var("BINANCE_API_SECRET").ok(),
            move || env::var("BINANCE_PRIVATE_KEY_FILE").ok(),
            move || env::var("BINANCE_PRIVATE_KEY_PHRASE").ok(),
        )])
    };

    // ==== Configure
    // ----

    let order_id_config = TimestampOrderIdsConfig::builder()
        .build_arc()
        .expect("Failed to build order ID provider");

    let timestamp_order_ids = order_id_config.expect_timestamp_order_ids_cloned();

    tracing::info!("Loding configuration...");

    let basket_manager_config = BasketManagerConfig::builder()
        .with_config_file(format!("{}/BasketManagerConfig.json", config_path))
        .build()
        .await
        .expect("Failed to build basket manager");

    let index_symbols = basket_manager_config.get_index_symbols();
    let asset_symbols = basket_manager_config.get_underlying_asset_symbols();

    let market_data_config = MarketDataConfig::builder()
        .zero_threshold(zero_threshold)
        .subscriptions(
            asset_symbols
                .iter()
                .map(|s| Subscription::new(s.clone(), Symbol::from("Binance")))
                .collect_vec(),
        )
        .with_price_tracker(true)
        .with_book_manager(true)
        .build()
        .expect("Failed to build market data");

    let price_tracker = market_data_config.expect_price_tracker_cloned();
    let fee_calculator = BinanceFeeCalculator::new(
        market_data_config.expect_price_tracker_exchange_rates_cloned(),
        main_quote_currency.clone(),
    );

    let order_sender_config = OrderSenderConfig::builder()
        .credentials(credentials)
        .symbols(asset_symbols.clone())
        .with_binance_fee_calculator(fee_calculator)
        .build()
        .expect("Failed to build order sender");

    let router_config = CollateralRouterConfig::builder()
        .build()
        .expect("Failed to build collateral router");

    let chain_mode = ChainMode::new_with_router(
        cli.simulate_chain,
        cli.rpc_urls,
        main_quote_currency,
        index_symbols,
        &market_data_config,
        format!("{}/index_maker.json", config_path),
        &router_config,
    )
    .await;

    let mint_invoice_manager_config = MintInvoiceManagerConfig::builder()
        .build()
        .expect("Failed to build mint invoice manager config");

    let index_order_manager_config = IndexOrderManagerConfig::builder()
        .zero_threshold(zero_threshold)
        .with_server(app_mode.get_server_config())
        .with_invoice_manager(mint_invoice_manager_config.clone())
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

    let query_service_config = cli.query_bind_address.map(|query_bind_address| {
        QueryServiceConfig::builder()
            .with_collateral_manager(collateral_manager_config.expect_collateral_manager_cloned())
            .with_index_order_manager(
                index_order_manager_config.expect_index_order_manager_cloned(),
            )
            .with_inventory_manager(order_sender_config.expect_inventory_manager_cloned())
            .with_invoice_manager(mint_invoice_manager_config.expect_invoice_manager_cloned())
            .address(query_bind_address)
            .build()
            .expect("Failed to build query service")
    });

    let strategy_config = SimpleSolverConfig::builder()
        .price_threshold(price_threshold)
        .max_levels(max_levels)
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
        .with_chain_connector(chain_mode.get_chain_connector_config())
        .build()
        .expect("Failed to build solver");

    if cli.dry_run {
        tracing::info!("✅ Dry run complete");
        return Ok(());
    }

    tracing::info!("Starting application threads...");
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

    tracing::info!("Connecting to blockchain...");
    chain_mode
        .run()
        .await
        .expect("Failed to start chain connector");

    tracing::info!("Starting FIX server...");
    app_mode.run().await;

    tracing::info!("Starting query service...");
    if let Some(query_service_config) = query_service_config {
        query_service_config.start().await?;

        tracing::info!(
            "Running query service at {}",
            query_service_config.address.unwrap()
        );
    }

    tracing::info!("Awaiting market data...");
    loop {
        sleep(std::time::Duration::from_secs(1)).await;
        let result = price_tracker
            .read()
            .get_prices(PriceType::BestAsk, &asset_symbols);

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
            if let ChainMode::Simulated { chain_config } = &chain_mode {
                tracing::info!("Sending deposit...");

                let simple_chain = chain_config.expect_chain_connector_cloned();

                simple_chain
                    .write()
                    .expect("Failed to lock chain connector")
                    .publish_event(ChainNotification::Deposit {
                        chain_id: 1,
                        address: get_mock_address_1(),
                        seq_num: U256::ONE,
                        amount: *collateral_amount,
                        affiliate1: None,
                        affiliate2: None,
                        timestamp: Utc::now(),
                    });

                sleep(std::time::Duration::from_secs(2)).await;
            }

            tracing::info!("Sending index order...");

            simple_server
                .write()
                .notify_server_event(&Arc::new(ServerEvent::NewIndexOrder {
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
            if let ChainMode::Simulated { chain_config } = &chain_mode {
                tracing::info!("Sending deposit...");

                let simple_chain = chain_config.expect_chain_connector_cloned();

                // Default addresses from Anvil. To get private keys run Anvil.
                let simulated_deposit_receipients = vec![
                    (1, address!("0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266")),
                    (1, address!("0x70997970C51812dc3A010C7d01b50e0d17dc79C8")),
                    (1, address!("0x3C44CdDdB6a900fa2b585dd299e03d12FA4293BC")),
                    (1, address!("0x90F79bf6EB2c4f870365E785982E1f101E93b906")),
                    (1, address!("0x15d34AAf54267DB7D7c367839AAf71A00a2C6A65")),
                    (1, address!("0x9965507D1a55bcC2695C58ba16FB37d819B0A4dc")),
                    (1, address!("0x976EA74026E726554dB657fA54763abd0C3a0aa9")),
                    (1, address!("0x14dC79964da2C08b23698B3D3cc7Ca32193d9955")),
                    (1, address!("0x23618e81E3f5cdF7f54C3d65f7FBc0aBf5B21E8f")),
                    (1, address!("0xa0Ee7A142d267C1f36714E4a8F75612F20a79720")),
                ];

                for (chain_id, address) in simulated_deposit_receipients {
                    simple_chain
                        .write()
                        .expect("Failed to lock chain connector")
                        .publish_event(ChainNotification::Deposit {
                            chain_id,
                            address,
                            seq_num: U256::ONE,
                            affiliate1: None,
                            affiliate2: None,
                            amount: *collateral_amount,
                            timestamp: Utc::now(),
                        });
                }
            }

            tracing::info!("Awaiting index order... (Please, send NewIndexOrder message to FIX server running at: {:?})",
                fix_server_config.address);
        }
        AppMode::QuoteServer { fix_server_config } => {
            tracing::info!("Awaiting quote request... (Please, send NewQuoteRequest message to FIX server running at: {:?})",
                fix_server_config.address);
        }
    };

    let mut sigint = signal(SignalKind::interrupt())?;
    let mut sigterm = signal(SignalKind::terminate())?;
    let mut sigquit = signal(SignalKind::quit())?;

    tokio::select! {
        _ = solver_config.check_solver_stopped() => {
            panic!("Solver terminated unexpectedly");
        }
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
    chain_mode
        .stop()
        .await
        .expect("Failed to stop chain connector");

    tracing::info!("Done.");

    Ok(())
}
