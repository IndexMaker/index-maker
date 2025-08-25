use std::sync::Arc;
use eyre::Result;
use tokio::signal::unix::{signal, SignalKind};
use chrono::Utc;

use crate::app::{
    config::{ApplicationConfig, ConfigBuildError},
    basket_manager::BasketManagerConfig,
    batch_manager::BatchManagerConfig,
    chain_connector::RealChainConnectorConfig,
    collateral_manager::CollateralManagerConfig,
    collateral_router::CollateralRouterConfig,
    fix_server::FixServerConfig,
    index_order_manager::IndexOrderManagerConfig,
    market_data::MarketDataConfig,
    order_sender::{OrderSenderConfig, OrderSenderCredentials},
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
};

use symm_core::{
    core::{
        bits::{Amount, PriceType, Side, Symbol},
        logging::log_init,
    },
    order_sender::order_connector::SessionId,
};

use parking_lot::RwLock;
use binance_order_sending::credentials::Credentials;

/// Main application that manages the lifecycle of all components
pub struct Application {
    config: ApplicationConfig,
    solver_config: Option<SolverConfig>,
    app_mode: Option<AppMode>,
    chain_mode: Option<ChainMode>,
}

/// Application mode enum that determines how the application runs
pub enum AppMode {
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

/// Chain mode enum for different chain connector configurations
pub enum ChainMode {
    Simulated {
        simple_chain_config: Arc<SimpleChainConnectorConfig>,
    },
    Real {
        real_chain_config: Arc<RealChainConnectorConfig>,
    },
}

/// Application builder that provides a fluent interface for configuring the application
pub struct ApplicationBuilder {
    config: ApplicationConfig,
}

impl ApplicationBuilder {
    /// Create a new application builder with the provided configuration
    pub fn new(config: ApplicationConfig) -> Self {
        Self { config }
    }

    /// Create a new application builder with default configuration
    pub fn with_defaults() -> Self {
        Self {
            config: ApplicationConfig::default(),
        }
    }

    /// Override the main quote currency
    pub fn with_main_quote_currency(mut self, currency: Symbol) -> Self {
        self.config.app.main_quote_currency = currency;
        self
    }

    /// Enable or disable sender simulation
    pub fn with_simulate_sender(mut self, simulate: bool) -> Self {
        self.config.app.simulate_sender = simulate;
        self
    }

    /// Enable or disable chain simulation
    pub fn with_simulate_chain(mut self, simulate: bool) -> Self {
        self.config.app.simulate_chain = simulate;
        self
    }

    /// Set the bind address for the server
    pub fn with_bind_address(mut self, address: String) -> Self {
        self.config.server.bind_address = address;
        self
    }

    /// Set the log level
    pub fn with_log_level(mut self, level: String) -> Self {
        self.config.logging.level = level;
        self
    }

    /// Set the log file path
    pub fn with_log_file<P: Into<std::path::PathBuf>>(mut self, path: P) -> Self {
        self.config.logging.file_path = Some(path.into());
        self
    }

    /// Set OTLP trace URL
    pub fn with_otlp_trace_url(mut self, url: String) -> Self {
        self.config.logging.otlp_trace_url = Some(url);
        self
    }

    /// Set OTLP log URL
    pub fn with_otlp_log_url(mut self, url: String) -> Self {
        self.config.logging.otlp_log_url = Some(url);
        self
    }

    /// Build the application with the configured settings
    pub async fn build(self) -> Result<Application, ConfigBuildError> {
        // Initialize logging first
        self.init_logging();

        // Log the configuration being used
        tracing::info!("Building application with configuration: {:#?}", self.config);

        let mut app = Application {
            config: self.config,
            solver_config: None,
            app_mode: None,
            chain_mode: None,
        };

        // Build all components
        app.build_components().await?;

        Ok(app)
    }

    /// Initialize logging based on configuration
    fn init_logging(&self) {
        let log_filter = format!("{}=info,Binance=off", env!("CARGO_CRATE_NAME"));
        
        log_init(
            log_filter,
            self.config.logging.file_path.as_ref().map(|p| p.to_string_lossy().to_string()),
            self.config.logging.disable_terminal,
            self.config.logging.otlp_trace_url.clone(),
            self.config.logging.otlp_log_url.clone(),
            Some(self.config.logging.batch_size),
        );
    }
}

impl Application {
    /// Build all application components based on configuration
    async fn build_components(&mut self) -> Result<(), ConfigBuildError> {
        // This method will contain the logic currently in main()
        // but organized and configurable through the ApplicationConfig
        
        // For now, we'll implement a basic structure
        // The full implementation will be added in the next step
        
        tracing::info!("Building application components...");
        
        // TODO: Implement component building logic
        // This will include:
        // - Building solver configuration
        // - Setting up market data
        // - Configuring chain connectors
        // - Setting up order senders
        // - etc.
        
        Ok(())
    }

    /// Run the application
    pub async fn run(&mut self) -> Result<()> {
        tracing::info!("Starting application...");

        // TODO: Implement the main application loop
        // This will replace the logic currently in main()
        
        // Set up signal handling
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

        tracing::info!("Stopping application...");
        self.stop().await?;

        Ok(())
    }

    /// Stop the application gracefully
    pub async fn stop(&mut self) -> Result<()> {
        tracing::info!("Stopping application components...");

        // TODO: Implement graceful shutdown logic
        // This will include stopping all components in the correct order

        Ok(())
    }

    /// Get the application configuration
    pub fn config(&self) -> &ApplicationConfig {
        &self.config
    }
}

impl AppMode {
    /// Create AppMode from command and configuration
    pub fn new(command: &crate::Commands, bind_address: String) -> Self {
        match command {
            crate::Commands::SendOrder {
                side,
                symbol,
                collateral_amount,
            } => {
                // TODO: Build SimpleServerConfig and SimpleServer
                // For now, we'll create placeholder implementations
                todo!("Implement SendOrder mode")
            }
            crate::Commands::FixServer { collateral_amount } => {
                // TODO: Build FixServerConfig
                todo!("Implement FixServer mode")
            }
            crate::Commands::QuoteServer {} => {
                // TODO: Build FixServerConfig for quotes
                todo!("Implement QuoteServer mode")
            }
        }
    }

    /// Run the application mode
    pub async fn run(&self) {
        match self {
            AppMode::SendOrder { .. } => {
                tracing::info!("Running in SendOrder mode");
                // TODO: Implement SendOrder logic
            }
            AppMode::FixServer { .. } => {
                tracing::info!("Running in FixServer mode");
                // TODO: Implement FixServer logic
            }
            AppMode::QuoteServer { .. } => {
                tracing::info!("Running in QuoteServer mode");
                // TODO: Implement QuoteServer logic
            }
        }
    }
}

impl ChainMode {
    /// Create ChainMode from configuration
    pub fn new_with_router(
        simulate_chain: bool,
        main_quote_currency: Symbol,
        router_config: &CollateralRouterConfig,
    ) -> Self {
        if simulate_chain {
            // TODO: Build SimpleChainConnectorConfig
            todo!("Implement simulated chain mode")
        } else {
            // TODO: Build RealChainConnectorConfig
            todo!("Implement real chain mode")
        }
    }

    /// Get chain connector configuration
    pub fn get_chain_connector_config(&self) -> Arc<dyn ChainConnectorConfig + Send + Sync> {
        match self {
            ChainMode::Simulated { simple_chain_config } => {
                simple_chain_config.clone() as Arc<dyn ChainConnectorConfig + Send + Sync>
            }
            ChainMode::Real { real_chain_config } => {
                real_chain_config.clone() as Arc<dyn ChainConnectorConfig + Send + Sync>
            }
        }
    }

    /// Run the chain mode
    pub async fn run(&self) -> Result<()> {
        match self {
            ChainMode::Simulated { .. } => {
                tracing::info!("Running simulated chain connector");
                // TODO: Implement simulated chain logic
            }
            ChainMode::Real { .. } => {
                tracing::info!("Running real chain connector");
                // TODO: Implement real chain logic
            }
        }
        Ok(())
    }
}
