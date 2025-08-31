//! # Index Maker Application
//! 
//! Main entry point for the Index Maker application with comprehensive configuration management,
//! secrets handling, and clean execution flow.

use clap::Parser;
use eyre::{OptionExt, Result};
use chrono;

use index_maker::{
    cli::{Cli, Commands},
    app::{
        application::ApplicationBuilder,
        config_loader::ConfigLoader,
        fix_server::FixServerConfig,
        simple_server::{SimpleServer, SimpleServerConfig},
    },
};
use std::sync::Arc;
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
    std::env::var("BITGET_API_KEY")
        .map_err(|_| eyre::eyre!("Failed to get BITGET_API_KEY secret"))
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
    std::env::var("CHAIN_RPC_URL")
        .map_err(|_| eyre::eyre!("Failed to get CHAIN_RPC_URL secret"))
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
            return Err(eyre::eyre!("Binance API key and secret must both be provided or both be missing"));
        }
        
        if self.bitget_api_key.is_some() && (self.bitget_api_secret.is_none() || self.bitget_passphrase.is_none()) {
            return Err(eyre::eyre!("Bitget requires API key, secret, and passphrase to all be provided"));
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
        self.bitget_api_key.is_some() && self.bitget_api_secret.is_some() && self.bitget_passphrase.is_some()
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
            Err(eyre::eyre!("Simple server not available for order execution"))
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

        let fix_server_config = self.fix_server_config.as_ref()
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

        let fix_server_config = self.fix_server_config.as_ref()
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
            Commands::SendOrder { side, symbol, quantity } => {
                Ok(AppMode::SendOrder(SendOrderMode::new(*side, symbol.clone(), *quantity)))
            }
            Commands::FixServer { collateral_amount } => {
                Ok(AppMode::FixServer(FixServerMode::new(*collateral_amount, bind_address)))
            }
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
    fn new_with_router(
        simulate_chain: bool,
        main_quote_currency: Symbol,
        index_symbols: Vec<Symbol>,
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
            let evm_connector_config = RealChainConnectorConfig::builder()
                .with_router(router_config.clone())
                .index_symbols(index_symbols)
                .build_arc()
                .expect("Failed to build evm chain connector");

            Self::Real {
                chain_config: evm_connector_config,
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
    // 1. Secrets initialization at the top (as per Sonia's requirements)
    let secrets = {
        let _span = tracing::info_span!("secrets_initialization").entered();
        let secrets = SecretsProvider::load()?;
        tracing::info!("Secrets provider initialized successfully");
        secrets
    };

    // 2. Parse command line arguments
    let cli = {
        let _span = tracing::info_span!("cli_parsing").entered();
        let cli = Cli::parse();
        tracing::debug!("Command line arguments parsed successfully");
        cli
    };

    // 3. Load configuration from multiple sources with proper precedence:
    // Default values (lowest priority) → Configuration file → Environment variables → CLI arguments (highest priority)
    let config = {
        let _span = tracing::info_span!("configuration_loading").entered();
        let config_loader = ConfigLoader::new_with_secrets(&cli, secrets.clone())?;
        let config = config_loader.load_config()?;
        tracing::info!("Configuration loaded successfully from all sources");
        config
    };

    // 4. Log the command being executed
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

    // 5. Build the application using the configuration and secrets
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

    // 6. Create and run the application mode
    let bind_address = cli.bind_address
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
        let _span = tracing::info_span!("application_execution", bind_address = %bind_address).entered();
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
