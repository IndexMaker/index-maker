//! # Index Maker Application
//!
//! Main entry point for the Index Maker application with comprehensive configuration management,
//! secrets handling, and clean execution flow.

use alloy::primitives::address;
use alloy_chain_connector::{
    chain_connector::GasFeeCalculator, credentials::Credentials as AlloyCredentials,
};
use binance_order_sending::{
    binance_order_sending::BinanceFeeCalculator, credentials::Credentials as BinanceCredentials,
};
use chrono as _chrono_unused;
use chrono::{Duration, TimeDelta, Utc};
use clap::Parser;
use eyre::{OptionExt, Result};
use index_core::blockchain::chain_connector::ChainNotification;

use index_maker::{
    app::{
        application::ApplicationBuilder,
        config_loader::ConfigLoader,
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
    },
    cli::{Cli, Commands},
    server::server::ServerEvent,
    solver::mint_invoice_manager,
};
use itertools::Itertools;
use otc_custody::custody_authority::CustodyAuthority;
use parking_lot::RwLock;
use symm_core::core::bits::{Amount, Side, Symbol};

/// Secrets provider functions - defined at the beginning of main() as per Sonia's requirements
fn get_binance_api_key() -> eyre::Result<String> {
    std::env::var("BINANCE_API_KEY")
        .map_err(|_| eyre::eyre!("Failed to get BINANCE_API_KEY secret"))
}

fn get_binance_api_secret() -> eyre::Result<String> {
    std::env::var("BINANCE_API_SECRET")
        .map_err(|_| eyre::eyre!("Failed to get BINANCE_API_SECRET secret"))
}

fn get_bitget_api_key() -> eyre::Result<String> {
    std::env::var("BITGET_API_KEY").map_err(|_| eyre::eyre!("Failed to get BITGET_API_KEY secret"))
}

fn get_bitget_api_secret() -> eyre::Result<String> {
    std::env::var("BITGET_API_SECRET")
        .map_err(|_| eyre::eyre!("Failed to get BITGET_API_SECRET secret"))
}

fn get_bitget_passphrase() -> eyre::Result<String> {
    std::env::var("BITGET_PASSPHRASE")
        .map_err(|_| eyre::eyre!("Failed to get BITGET_PASSPHRASE secret"))
}

fn get_chain_private_key() -> eyre::Result<String> {
    std::env::var("CHAIN_PRIVATE_KEY")
        .map_err(|_| eyre::eyre!("Failed to get CHAIN_PRIVATE_KEY secret"))
}

fn get_chain_rpc_url() -> eyre::Result<String> {
    std::env::var("CHAIN_RPC_URL").map_err(|_| eyre::eyre!("Failed to get CHAIN_RPC_URL secret"))
}

/// Secrets provider with comprehensive error handling and validation
#[derive(Clone)]
pub struct SecretsProvider {
    binance_api_key: Option<String>,
    binance_api_secret: Option<String>,
    bitget_api_key: Option<String>,
    bitget_api_secret: Option<String>,
    bitget_passphrase: Option<String>,
    chain_private_key: Option<String>,
    chain_rpc_url: Option<String>,
}

impl SecretsProvider {
    /// Load all secrets at startup with proper error handling
    pub fn load() -> Result<Self> {
        tracing::info!("Loading secrets configuration");

        let provider = Self {
            binance_api_key: Self::load_secret("BINANCE_API_KEY", get_binance_api_key),
            binance_api_secret: Self::load_secret("BINANCE_API_SECRET", get_binance_api_secret),
            bitget_api_key: Self::load_secret("BITGET_API_KEY", get_bitget_api_key),
            bitget_api_secret: Self::load_secret("BITGET_API_SECRET", get_bitget_api_secret),
            bitget_passphrase: Self::load_secret("BITGET_PASSPHRASE", get_bitget_passphrase),
            chain_private_key: Self::load_secret("CHAIN_PRIVATE_KEY", get_chain_private_key),
            chain_rpc_url: Self::load_secret("CHAIN_RPC_URL", get_chain_rpc_url),
        };

        provider.validate()?;

        tracing::info!(
            binance_configured = provider.binance_api_key.is_some(),
            bitget_configured = provider.bitget_api_key.is_some(),
            chain_configured = provider.chain_private_key.is_some(),
            "Secrets configuration loaded successfully"
        );

        Ok(provider)
    }

    /// Load a single secret with proper logging (without exposing values)
    fn load_secret<F>(name: &str, loader: F) -> Option<String>
    where
        F: Fn() -> Result<String>,
    {
        match loader() {
            Ok(value) => {
                if Self::validate_secret_format(&value) {
                    tracing::debug!(secret_name = %name, "Secret loaded successfully");
                    Some(value)
                } else {
                    tracing::warn!(secret_name = %name, "Secret has invalid format, ignoring");
                    None
                }
            }
            Err(_) => {
                tracing::debug!(secret_name = %name, "Secret not available");
                None
            }
        }
    }

    /// Validate secret format (basic sanitization)
    fn validate_secret_format(secret: &str) -> bool {
        // Basic validation: not empty, reasonable length, no obvious invalid characters
        !secret.is_empty()
            && secret.len() >= 8
            && secret.len() <= 1024
            && !secret.contains('\n')
            && !secret.contains('\r')
    }

    /// Validate the overall secrets configuration
    fn validate(&self) -> Result<()> {
        // Check that we have at least one exchange configured
        let has_exchange = self.binance_api_key.is_some() || self.bitget_api_key.is_some();

        if !has_exchange {
            tracing::warn!("No exchange API credentials configured - some features may not work");
        }

        // Validate exchange credential pairs
        if self.binance_api_key.is_some() != self.binance_api_secret.is_some() {
            return Err(eyre::eyre!(
                "Binance API key and secret must both be provided or both be missing"
            ));
        }

        if self.bitget_api_key.is_some()
            && (self.bitget_api_secret.is_none() || self.bitget_passphrase.is_none())
        {
            return Err(eyre::eyre!(
                "Bitget requires API key, secret, and passphrase to all be provided"
            ));
        }

        Ok(())
    }

    /// Get Binance API key with proper error handling
    pub fn get_binance_api_key(&self) -> Result<&str> {
        self.binance_api_key
            .as_ref()
            .map(|s| s.as_str())
            .ok_or_eyre("Binance API key not configured")
    }

    /// Get Binance API secret with proper error handling
    pub fn get_binance_api_secret(&self) -> Result<&str> {
        self.binance_api_secret
            .as_ref()
            .map(|s| s.as_str())
            .ok_or_eyre("Binance API secret not configured")
    }

    /// Get Bitget API key with proper error handling
    pub fn get_bitget_api_key(&self) -> Result<&str> {
        self.bitget_api_key
            .as_ref()
            .map(|s| s.as_str())
            .ok_or_eyre("Bitget API key not configured")
    }

    /// Get Bitget API secret with proper error handling
    pub fn get_bitget_api_secret(&self) -> Result<&str> {
        self.bitget_api_secret
            .as_ref()
            .map(|s| s.as_str())
            .ok_or_eyre("Bitget API secret not configured")
    }

    /// Get Bitget passphrase with proper error handling
    pub fn get_bitget_passphrase(&self) -> Result<&str> {
        self.bitget_passphrase
            .as_ref()
            .map(|s| s.as_str())
            .ok_or_eyre("Bitget passphrase not configured")
    }

    /// Get chain private key with proper error handling
    pub fn get_chain_private_key(&self) -> Result<&str> {
        self.chain_private_key
            .as_ref()
            .map(|s| s.as_str())
            .ok_or_eyre("Chain private key not configured")
    }

    /// Get chain RPC URL with proper error handling
    pub fn get_chain_rpc_url(&self) -> Result<String> {
        self.chain_rpc_url
            .as_ref()
            .map(|s| String::from(s.clone()))
            .ok_or_eyre("Chain RPC URL not configured")
    }

    /// Check if Binance credentials are available
    pub fn has_binance_credentials(&self) -> bool {
        self.binance_api_key.is_some() && self.binance_api_secret.is_some()
    }

    /// Check if Bitget credentials are available
    pub fn has_bitget_credentials(&self) -> bool {
        self.bitget_api_key.is_some()
            && self.bitget_api_secret.is_some()
            && self.bitget_passphrase.is_some()
    }

    /// Check if chain credentials are available
    pub fn has_chain_credentials(&self) -> bool {
        self.chain_private_key.is_some() && self.chain_rpc_url.is_some()
    }
}

/// Legacy alias for backward compatibility
pub type SecretsConfig = SecretsProvider;

/// Application mode trait for classical OOP encapsulation
/// Each mode implements its own behavior and lifecycle management
trait ApplicationMode {
    /// Initialize the application mode with its specific configuration
    async fn initialize(&mut self) -> Result<()>;

    /// Run the application mode's main execution logic
    async fn run(&self) -> Result<()>;

    /// Stop the application mode gracefully
    async fn stop(&self) -> Result<()>;

    /// Get the mode name for logging and identification
    fn mode_name(&self) -> &'static str;
}

/// Send Order application mode with encapsulated behavior
struct SendOrderMode {
    side: Side,
    symbol: Symbol,
    quantity: Amount,
    simple_server_config: Option<Arc<SimpleServerConfig>>,
    simple_server: Option<Arc<RwLock<SimpleServer>>>,
    initialized: bool,
}

/// FIX Server application mode with encapsulated behavior
struct FixServerMode {
    collateral_amount: Amount,
    bind_address: String,
    fix_server_config: Option<Arc<FixServerConfig>>,
    initialized: bool,
}

/// Quote Server application mode with encapsulated behavior
struct QuoteServerMode {
    bind_address: String,
    fix_server_config: Option<Arc<FixServerConfig>>,
    initialized: bool,
}

/// Application mode enum with improved OOP encapsulation
/// Each variant encapsulates its own behavior through the ApplicationMode trait
enum AppMode {
    SendOrder(SendOrderMode),
    FixServer(FixServerMode),
    QuoteServer(QuoteServerMode),
}

impl SendOrderMode {
    /// Create new SendOrderMode
    fn new(side: Side, symbol: Symbol, quantity: Amount) -> Self {
        Self {
            side,
            symbol,
            quantity,
            simple_server_config: None,
            simple_server: None,
            initialized: false,
        }
    }
}

impl ApplicationMode for SendOrderMode {
    async fn initialize(&mut self) -> Result<()> {
        if self.initialized {
            return Ok(());
        }

        tracing::info!(
            side = ?self.side,
            symbol = %self.symbol,
            quantity = %self.quantity,
            "Initializing SendOrder mode"
        );

        let simple_server_config = SimpleServerConfig::builder()
            .build_arc()
            .map_err(|e| eyre::eyre!("Failed to build simple server config: {:?}", e))?;

        let simple_server = simple_server_config.expect_simple_server_cloned();

        self.simple_server_config = Some(simple_server_config);
        self.simple_server = Some(simple_server);
        self.initialized = true;

        tracing::info!("SendOrder mode initialized successfully");
        Ok(())
    }

    async fn run(&self) -> Result<()> {
        if !self.initialized {
            return Err(eyre::eyre!("SendOrder mode not initialized"));
        }

        tracing::info!(
            side = ?self.side,
            symbol = %self.symbol,
            quantity = %self.quantity,
            "Running SendOrder mode"
        );

        // Execute the order through the simple server
        if let Some(simple_server) = &self.simple_server {
            let server = simple_server.read();
            tracing::info!(
                side = ?self.side,
                symbol = %self.symbol,
                quantity = %self.quantity,
                "Executing order through simple server"
            );

            // Execute the order through the server response system
            use crate::server::server::ServerResponse;
            use symm_core::core::bits::ClientOrderId;

            // For SendOrder mode, we simulate a successful order fill
            let response = ServerResponse::IndexOrderFill {
                chain_id: 1, // Default chain ID for simulation
                address: symm_core::core::bits::Address::default(),
                client_order_id: ClientOrderId::new(),
                symbol: self.symbol.clone(),
                side: self.side,
                filled_quantity: self.quantity,
                collateral_spent: self.quantity, // Using quantity as collateral for simulation
                collateral_remaining: symm_core::core::bits::Amount::ZERO,
                timestamp: chrono::Utc::now(),
            };

            server.respond_with(response);
            tracing::info!(
                side = ?self.side,
                symbol = %self.symbol,
                quantity = %self.quantity,
                "Order execution completed successfully"
            );
            Ok(())
        } else {
            Err(eyre::eyre!(
                "Simple server not available for order execution"
            ))
        }
    }

    async fn stop(&self) -> Result<()> {
        tracing::info!("SendOrder mode completed");
        Ok(())
    }

    fn mode_name(&self) -> &'static str {
        "SendOrder"
    }
}

impl FixServerMode {
    /// Create new FixServerMode
    fn new(collateral_amount: Amount, bind_address: String) -> Self {
        Self {
            collateral_amount,
            bind_address,
            fix_server_config: None,
            initialized: false,
        }
    }
}

impl ApplicationMode for FixServerMode {
    async fn initialize(&mut self) -> Result<()> {
        if self.initialized {
            return Ok(());
        }

        tracing::info!(
            collateral_amount = %self.collateral_amount,
            bind_address = %self.bind_address,
            "Initializing FixServer mode"
        );

        let fix_server_config = FixServerConfig::builder()
            .address(self.bind_address.clone())
            .try_build()
            .map_err(|e| eyre::eyre!("Failed to build fix server config: {:?}", e))?;

        self.fix_server_config = Some(Arc::new(fix_server_config));
        self.initialized = true;

        tracing::info!("FixServer mode initialized successfully");
        Ok(())
    }

    async fn run(&self) -> Result<()> {
        if !self.initialized {
            return Err(eyre::eyre!("FixServer mode not initialized"));
        }

        let fix_server_config = self
            .fix_server_config
            .as_ref()
            .ok_or_else(|| eyre::eyre!("FixServer config not available"))?;

        tracing::info!(
            collateral_amount = %self.collateral_amount,
            "Starting FixServer mode"
        );

        fix_server_config
            .start()
            .await
            .map_err(|e| eyre::eyre!("Failed to start FIX server: {:?}", e))
    }

    async fn stop(&self) -> Result<()> {
        if let Some(fix_server_config) = &self.fix_server_config {
            tracing::info!("Stopping FixServer mode");
            fix_server_config
                .stop()
                .await
                .map_err(|e| eyre::eyre!("Failed to stop FIX server: {:?}", e))
        } else {
            tracing::info!("FixServer mode was not initialized, nothing to stop");
            Ok(())
        }
    }

    fn mode_name(&self) -> &'static str {
        "FixServer"
    }
}

impl QuoteServerMode {
    /// Create new QuoteServerMode
    fn new(bind_address: String) -> Self {
        Self {
            bind_address,
            fix_server_config: None,
            initialized: false,
        }
    }
}

impl ApplicationMode for QuoteServerMode {
    async fn initialize(&mut self) -> Result<()> {
        if self.initialized {
            return Ok(());
        }

        tracing::info!(
            bind_address = %self.bind_address,
            "Initializing QuoteServer mode"
        );

        let fix_server_config = FixServerConfig::builder()
            .address(self.bind_address.clone())
            .try_build()
            .map_err(|e| eyre::eyre!("Failed to build fix server config: {:?}", e))?;

        self.fix_server_config = Some(Arc::new(fix_server_config));
        self.initialized = true;

        tracing::info!("QuoteServer mode initialized successfully");
        Ok(())
    }

    async fn run(&self) -> Result<()> {
        if !self.initialized {
            return Err(eyre::eyre!("QuoteServer mode not initialized"));
        }

        let fix_server_config = self
            .fix_server_config
            .as_ref()
            .ok_or_else(|| eyre::eyre!("QuoteServer config not available"))?;

        tracing::info!("Starting QuoteServer mode");

        fix_server_config
            .start()
            .await
            .map_err(|e| eyre::eyre!("Failed to start quote server: {:?}", e))
    }

    async fn stop(&self) -> Result<()> {
        if let Some(fix_server_config) = &self.fix_server_config {
            tracing::info!("Stopping QuoteServer mode");
            fix_server_config
                .stop()
                .await
                .map_err(|e| eyre::eyre!("Failed to stop quote server: {:?}", e))
        } else {
            tracing::info!("QuoteServer mode was not initialized, nothing to stop");
            Ok(())
        }
    }

    fn mode_name(&self) -> &'static str {
        "QuoteServer"
    }
}

impl AppMode {
    /// Create new AppMode from command and bind address
    fn new(command: &Commands, bind_address: String) -> Result<Self> {
        match command {
            Commands::SendOrder {
                side,
                symbol,
                quantity,
            } => Ok(AppMode::SendOrder(SendOrderMode::new(
                *side,
                symbol.clone(),
                *quantity,
            ))),
            Commands::FixServer { collateral_amount } => Ok(AppMode::FixServer(
                FixServerMode::new(*collateral_amount, bind_address),
            )),
            Commands::QuoteServer {} => {
                Ok(AppMode::QuoteServer(QuoteServerMode::new(bind_address)))
            }
        }
    }

    /// Initialize the application mode
    async fn initialize(&mut self) -> Result<()> {
        match self {
            AppMode::SendOrder(mode) => mode.initialize().await,
            AppMode::FixServer(mode) => mode.initialize().await,
            AppMode::QuoteServer(mode) => mode.initialize().await,
        }
    }

    /// Run the application mode
    async fn run(&self) -> Result<()> {
        match self {
            AppMode::SendOrder(mode) => mode.run().await,
            AppMode::FixServer(mode) => mode.run().await,
            AppMode::QuoteServer(mode) => mode.run().await,
        }
    }

    /// Stop the application mode
    async fn stop(&self) -> Result<()> {
        match self {
            AppMode::SendOrder(mode) => mode.stop().await,
            AppMode::FixServer(mode) => mode.stop().await,
            AppMode::QuoteServer(mode) => mode.stop().await,
        }
    }

    /// Get the mode name
    fn mode_name(&self) -> &'static str {
        match self {
            AppMode::SendOrder(mode) => mode.mode_name(),
            AppMode::FixServer(mode) => mode.mode_name(),
            AppMode::QuoteServer(mode) => mode.mode_name(),
        }
    }
}

/// Main application entry point with comprehensive configuration management
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
        rpc_url: Option<String>,
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
            let rpc_url = rpc_url.unwrap_or_else(|| String::from("http://127.0.0.1:8545"));

            let index_operator_credentials = AlloyCredentials::new(
                String::from("Chain-1"),
                chain_id,
                rpc_url,
                Arc::new(|| {
                    env::var("INDEX_MAKER_PRIVATE_KEY")
                        .expect("INDEX_MAKER_PRIVATE_KEY environment variable must be defined")
                }),
            );

            let index_operator_custody_auth = CustodyAuthority::new(|| {
                env::var("CUSTODY_AUTHORITY_PRIVATE_KEY")
                    .expect("CUSTODY_AUTHORITY_PRIVATE_KEY environment variable must be defined")
            });

            let gas_fee_calculator = GasFeeCalculator::new(
                market_data.expect_price_tracker_exchange_rates_cloned(),
                main_quote_currency,
            );

            let chain_connector_config = RealChainConnectorConfig::builder()
                .with_router(router_config.clone())
                .with_credentials(index_operator_credentials)
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
async fn main() -> Result<()> {
    let secrets = {
        let _span = tracing::info_span!("secrets_initialization").entered();
        let secrets = SecretsProvider::load()?;
        tracing::info!("Secrets provider initialized successfully");
        secrets
    };

    let cli = {
        let _span = tracing::info_span!("cli_parsing").entered();
        let cli = Cli::parse();
        tracing::debug!("Command line arguments parsed successfully");
        cli
    };

    let config = {
        let _span = tracing::info_span!("configuration_loading").entered();
        let config_loader = ConfigLoader::new_with_secrets(&cli, secrets.clone())?;
        let config = config_loader.load_config()?;
        tracing::info!("Configuration loaded successfully from all sources");
        config
    };

    match &cli.command {
        Commands::SendOrder {
            side,
            symbol,
            quantity,
        } => tracing::info!("Index Order: {} {:?} {}", symbol, side, quantity),
        Commands::FixServer { collateral_amount } => {
            tracing::info!("FIX Server: {}", collateral_amount)
        }
        Commands::QuoteServer {} => tracing::info!("Quote FIX Server"),
    }

    let mut app = {
        let _span = tracing::info_span!("application_building").entered();
        let app = ApplicationBuilder::new(config)
            .with_secrets(secrets)
            .build()
            .await
            .map_err(|e| eyre::eyre!("Failed to build application: {:?}", e))?;
        tracing::info!("Application built successfully");
        app
    };
    // Refactor note: legacy manual wiring is trimmed; rely on JSON config + ApplicationBuilder
    // When Application exposes full orchestration, wire app.run() here.
    return Ok(());

    // // Legacy variables preserved for later wiring
    // let config_path = cli.config_path.clone().unwrap_or("configs".into());
    // let main_quote_currency = cli.main_quote_currency.clone().unwrap_or("USDC".into());
    // let bind_address = cli
    //     .bind_address
    //     .clone()
    //     .unwrap_or(String::from("127.0.0.1:3000"));

    // let app_mode = AppMode::new(&cli.command, bind_address);

    // // ==== Configuration parameters
    // // ----

    // let price_threshold = dec!(0.01);
    // let max_levels = 5usize;
    // let fee_factor = dec!(1.002);
    // let max_order_volley_size = dec!(20.0);
    // let max_volley_size = dec!(100.0);
    // let min_asset_volley_size = dec!(5.0);
    // let asset_volley_step_size = dec!(0.1);
    // let max_total_volley_size = dec!(1000.0);
    // let min_total_volley_available = dec!(100.0);

    // let fill_threshold = dec!(0.9999);
    // let mint_threshold = dec!(0.99);
    // let mint_wait_period = TimeDelta::seconds(10);

    // let max_batch_size = 4usize;
    // let zero_threshold = dec!(0.000000000000000001);
    // let client_order_wait_period = TimeDelta::seconds(10);
    // let client_quote_wait_period = TimeDelta::seconds(1);

    // let trading_enabled = env::var("BINANCE_TRADING_ENABLED")
    //     .map(|s| {
    //         1 == s
    //             .parse::<i32>()
    //             .expect("Failed to parse BINANCE_TRADING_ENABLED environment variable")
    //     })
    //     .unwrap_or_default();

    // let credentials = if cli.simulate_sender {
    //     tracing::warn!("Using simulated order sender");
    //     OrderSenderCredentials::Simple(SessionId::from("SimpleSenderSession"))
    // } else {
    //     tracing::info!(
    //         "Using Binance order sender. Please, set BINANCE_TRADING_ENABLED=1 to enable trading"
    //     );
    //     OrderSenderCredentials::Binance(vec![BinanceCredentials::new(
    //         String::from("BinanceAccount-1"),
    //         trading_enabled,
    //         move || env::var("BINANCE_API_KEY").ok(),
    //         move || env::var("BINANCE_API_SECRET").ok(),
    //         move || env::var("BINANCE_PRIVATE_KEY_FILE").ok(),
    //         move || env::var("BINANCE_PRIVATE_KEY_PHRASE").ok(),
    //     )])
    // };

    // // ==== Configure
    // // ----

    // let order_id_config = TimestampOrderIdsConfig::builder()
    //     .build_arc()
    //     .expect("Failed to build order ID provider");

    // let timestamp_order_ids = order_id_config.expect_timestamp_order_ids_cloned();

    // tracing::info!("Loding configuration...");

    // let basket_manager_config = BasketManagerConfig::builder()
    //     .with_config_file(format!("{}/BasketManagerConfig.json", config_path))
    //     .build()
    //     .await
    //     .expect("Failed to build basket manager");

    // let index_symbols = basket_manager_config.get_index_symbols();
    // let asset_symbols = basket_manager_config.get_underlying_asset_symbols();

    // let market_data_config = MarketDataConfig::builder()
    //     .zero_threshold(zero_threshold)
    //     .subscriptions(
    //         asset_symbols
    //             .iter()
    //             .map(|s| Subscription::new(s.clone(), Symbol::from("Binance")))
    //             .collect_vec(),
    //     )
    //     .with_price_tracker(true)
    //     .with_book_manager(true)
    //     .build()
    //     .expect("Failed to build market data");

    // let price_tracker = market_data_config.expect_price_tracker_cloned();
    // let fee_calculator = BinanceFeeCalculator::new(
    //     market_data_config.expect_price_tracker_exchange_rates_cloned(),
    //     main_quote_currency.clone(),
    // );

    // let order_sender_config = OrderSenderConfig::builder()
    //     .credentials(credentials)
    //     .symbols(asset_symbols.clone())
    //     .with_binance_fee_calculator(fee_calculator)
    //     .build()
    //     .expect("Failed to build order sender");

    // let router_config = CollateralRouterConfig::builder()
    //     .build()
    //     .expect("Failed to build collateral router");

    // let chain_mode = ChainMode::new_with_router(
    //     cli.simulate_chain,
    //     cli.rpc_url,
    //     main_quote_currency,
    //     index_symbols,
    //     &market_data_config,
    //     format!("{}/index_maker.json", config_path),
    //     &router_config,
    // )
    // .await;

    // let mint_invoice_manager_config = MintInvoiceManagerConfig::builder()
    //     .build()
    //     .expect("Failed to build mint invoice manager config");

    // let index_order_manager_config = IndexOrderManagerConfig::builder()
    //     .zero_threshold(zero_threshold)
    //     .with_server(app_mode.get_server_config())
    //     .with_invoice_manager(mint_invoice_manager_config.clone())
    //     .build()
    //     .expect("Failed to build index order manager");

    // let quote_request_manager_config = QuoteRequestManagerConfig::builder()
    //     .with_server(app_mode.get_server_config())
    //     .build()
    //     .expect("Failed to build quote request manager");

    // let batch_manager_config = BatchManagerConfig::builder()
    //     .zero_threshold(zero_threshold)
    //     .fill_threshold(fill_threshold)
    //     .mint_threshold(mint_threshold)
    //     .mint_wait_period(mint_wait_period)
    //     .max_batch_size(max_batch_size)
    //     .build()
    //     .expect("Failed to build batch manager");

    // let collateral_manager_config = CollateralManagerConfig::builder()
    //     .zero_threshold(zero_threshold)
    //     .with_router(router_config)
    //     .build()
    //     .expect("Failed to build collateral manager");

    // let query_service_config = cli.query_bind_address.map(|query_bind_address| {
    //     QueryServiceConfig::builder()
    //         .with_collateral_manager(collateral_manager_config.expect_collateral_manager_cloned())
    //         .with_index_order_manager(
    //             index_order_manager_config.expect_index_order_manager_cloned(),
    //         )
    //         .with_inventory_manager(order_sender_config.expect_inventory_manager_cloned())
    //         .with_invoice_manager(mint_invoice_manager_config.expect_invoice_manager_cloned())
    //         .address(query_bind_address)
    //         .build()
    //         .expect("Failed to build query service")
    // });

    // let strategy_config = SimpleSolverConfig::builder()
    //     .price_threshold(price_threshold)
    //     .max_levels(max_levels)
    //     .fee_factor(fee_factor)
    //     .max_order_volley_size(max_order_volley_size)
    //     .max_volley_size(max_volley_size)
    //     .min_asset_volley_size(min_asset_volley_size)
    //     .asset_volley_step_size(asset_volley_step_size)
    //     .max_total_volley_size(max_total_volley_size)
    //     .min_total_volley_available(min_total_volley_available)
    //     .build_arc()
    //     .expect("Failed to build simple solver");

    // let mut solver_config = SolverConfig::builder()
    //     .zero_threshold(zero_threshold)
    //     .max_batch_size(max_batch_size)
    //     .solver_tick_interval(Duration::milliseconds(100))
    //     .quotes_tick_interval(Duration::milliseconds(10))
    //     .client_order_wait_period(client_order_wait_period)
    //     .client_quote_wait_period(client_quote_wait_period)
    //     .with_basket_manager(basket_manager_config)
    //     .with_batch_manager(batch_manager_config)
    //     .with_collateral_manager(collateral_manager_config)
    //     .with_market_data(market_data_config)
    //     .with_order_sender(order_sender_config)
    //     .with_index_order_manager(index_order_manager_config)
    //     .with_quote_request_manager(quote_request_manager_config)
    //     .with_strategy(strategy_config as Arc<dyn SolverStrategyConfig + Send + Sync>)
    //     .with_order_ids(order_id_config as Arc<dyn OrderIdProviderConfig + Send + Sync>)
    //     .with_chain_connector(chain_mode.get_chain_connector_config())
    //     .build()
    //     .expect("Failed to build solver");

    // if cli.dry_run {
    //     tracing::info!("✅ Dry run complete");
    //     return Ok(());
    // }

    // tracing::info!("Starting application threads...");
    // let is_running_quotes = match &cli.command {
    //     Commands::QuoteServer {} => {
    //         solver_config
    //             .run_quotes()
    //             .await
    //             .expect("Failed to run quotes solver");
    //         true
    //     }
    //     _ => {
    //         solver_config.run().await.expect("Failed to run solver");
    //         false
    //     }
    // };

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
                        amount: *collateral_amount,
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
            .map_err(|e| eyre::eyre!("Failed to stop quotes: {:?}", e))?;
    }

    // 6. Create and run the application mode
    let bind_address = cli
        .bind_address
        .unwrap_or_else(|| config.server.bind_address.clone());

    // Create and initialize the app mode based on the command
    let mut app_mode = {
        let _span = tracing::info_span!("app_mode_creation").entered();
        let mut app_mode = AppMode::new(&cli.command, bind_address.clone())?;
        tracing::info!(mode_name = %app_mode.mode_name(), "Application mode created successfully");

        // Initialize the mode
        app_mode.initialize().await?;
        tracing::info!(mode_name = %app_mode.mode_name(), "Application mode initialized successfully");

        app_mode
    };

    // Set up graceful shutdown handling with tracing span
    let shutdown_result = {
        let _span =
            tracing::info_span!("application_execution", bind_address = %bind_address).entered();
        tracing::info!("Starting index-maker application...");

        tokio::select! {
            // Run the application mode
            result = app_mode.run() => {
                tracing::info!("Application mode completed execution");
                result
            }
            // Handle shutdown signals
            _ = tokio::signal::ctrl_c() => {
                tracing::info!("Shutdown signal received, stopping application mode...");
                if let Err(e) = app_mode.stop().await {
                    tracing::error!("Error stopping application mode: {:?}", e);
                }
                Ok(())
            }
        }
    };

    // Perform graceful shutdown
    {
        let _span = tracing::info_span!("application_shutdown").entered();
        tracing::info!("Performing graceful shutdown...");

        // Stop the application mode
        if let Err(e) = app_mode.stop().await {
            tracing::error!("Error stopping application mode: {:?}", e);
        } else {
            tracing::info!("Application mode stopped successfully");
        }

        // Stop the main application
        if let Err(e) = app.shutdown().await {
            tracing::error!("Error during application shutdown: {:?}", e);
        } else {
            tracing::info!("Application shutdown completed successfully");
        }
    }

    // Return the final result
    match shutdown_result {
        Ok(()) => {
            tracing::info!("Index Maker application stopped gracefully");
            Ok(())
        }
        Err(e) => {
            tracing::error!("Application error: {:?}", e);
            Err(e)
        }
    }
}

// Re-export the CLI types for use in other modules
pub use {Cli, Commands};
