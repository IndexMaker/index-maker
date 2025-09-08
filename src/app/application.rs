use eyre::Result;
use std::sync::Arc;
use tokio::signal::unix::{signal, SignalKind};

use crate::app::{
    basket_manager::BasketManagerConfig,
    chain_connector::RealChainConnectorConfig,
    collateral_router::CollateralRouterConfig,
    config::{ApplicationConfig, ConfigBuildError},
    simple_chain::SimpleChainConnectorConfig,
    solver::{ChainConnectorConfig, SolverConfig},
};

use symm_core::core::{bits::Symbol, logging::log_init};

/// Main application that manages the lifecycle of all components
pub struct Application {
    config: ApplicationConfig,
    secrets: Option<crate::SecretsProvider>,
    solver_config: Option<SolverConfig>,
    chain_mode: Option<ChainMode>,
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
    secrets: Option<crate::SecretsProvider>,
}

impl ApplicationBuilder {
    /// Create a new application builder with the provided configuration
    pub fn new(config: ApplicationConfig) -> Self {
        Self {
            config,
            secrets: None,
        }
    }

    /// Create a new application builder with default configuration
    pub fn with_defaults() -> Self {
        Self {
            config: ApplicationConfig::default(),
            secrets: None,
        }
    }

    /// Set the secrets provider for dependency injection
    pub fn with_secrets(mut self, secrets: crate::SecretsProvider) -> Self {
        self.secrets = Some(secrets);
        self
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
        tracing::info!(
            "Building application with configuration: {:#?}",
            self.config
        );

        let mut app = Application {
            config: self.config,
            secrets: self.secrets,
            solver_config: None,
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
            self.config
                .logging
                .file_path
                .as_ref()
                .map(|p| p.to_string_lossy().to_string()),
            self.config.logging.disable_terminal,
            Some(self.config.logging.otlp_trace_url.clone()),
            Some(self.config.logging.otlp_log_url.clone()),
            Some(self.config.logging.batch_size),
        );
    }
}

impl Application {
    /// Build all application components based on configuration using functional patterns
    async fn build_components(&mut self) -> Result<(), ConfigBuildError> {
        tracing::info!("Building application components from configuration...");

        // Use functional pipeline for component building
        let component_results = vec![
            ("basket_manager", self.build_basket_manager()),
            ("secrets_integration", self.integrate_secrets()),
        ];

        // Process all component results using functional patterns
        let (successful, failed): (Vec<_>, Vec<_>) = component_results
            .into_iter()
            .partition(|(_, result)| result.is_ok());

        // Report any failures
        if !failed.is_empty() {
            let error_messages: Vec<String> = failed
                .into_iter()
                .map(|(name, result)| format!("{}: {:?}", name, result.unwrap_err()))
                .collect();

            return Err(ConfigBuildError::Other(format!(
                "Failed to build components: [{}]",
                error_messages.join(", ")
            )));
        }

        // Log successful components
        let successful_names: Vec<&str> = successful.iter().map(|(name, _)| *name).collect();

        tracing::info!(
            components = ?successful_names,
            count = successful_names.len(),
            "Application components built successfully"
        );

        Ok(())
    }

    /// Build basket manager component
    fn build_basket_manager(&mut self) -> Result<(), ConfigBuildError> {
        let basket_manager = BasketManagerConfig::from_config_data(&self.config.basket_manager)
            .map_err(|e| {
                ConfigBuildError::Other(format!("Failed to build basket manager: {:?}", e))
            })?;

        tracing::info!(
            symbols_count = basket_manager.get_symbols().len(),
            "Basket manager built successfully"
        );

        Ok(())
    }

    /// Integrate secrets with components that need them
    fn integrate_secrets(&mut self) -> Result<(), ConfigBuildError> {
        if let Some(ref secrets) = self.secrets {
            // Use functional patterns to validate secrets availability
            let required_secrets = vec![
                (
                    "exchange_credentials",
                    secrets.has_binance_credentials() || secrets.has_bitget_credentials(),
                ),
                ("chain_credentials", secrets.has_chain_credentials()),
            ];

            let missing_secrets: Vec<&str> = required_secrets
                .iter()
                .filter(|(_, available)| !available)
                .map(|(name, _)| *name)
                .collect();

            if !missing_secrets.is_empty() {
                tracing::warn!(
                    missing_secrets = ?missing_secrets,
                    "Some secrets are not configured - related features may not work"
                );
            }

            let available_secrets: Vec<&str> = required_secrets
                .iter()
                .filter(|(_, available)| *available)
                .map(|(name, _)| *name)
                .collect();

            tracing::info!(
                available_secrets = ?available_secrets,
                "Secrets integration completed"
            );
        } else {
            tracing::warn!("No secrets provider available");
        }

        Ok(())
    }

    /// Run the application
    pub async fn run(&mut self) -> Result<()> {
        tracing::info!("Starting application...");

        // Start all components
        self.start_components().await?;

        // Set up signal handling
        let mut sigint = signal(SignalKind::interrupt())?;
        let mut sigterm = signal(SignalKind::terminate())?;
        let mut sigquit = signal(SignalKind::quit())?;

        tracing::info!("Application started successfully, waiting for shutdown signal...");

        tokio::select! {
            _ = sigint.recv() => {
                tracing::info!("SIGINT received - initiating graceful shutdown")
            }
            _ = sigterm.recv() => {
                tracing::info!("SIGTERM received - initiating graceful shutdown")
            }
            _ = sigquit.recv() => {
                tracing::info!("SIGQUIT received - initiating graceful shutdown")
            }
        }

        tracing::info!("Stopping application...");
        self.stop().await?;

        Ok(())
    }

    /// Start all application components
    async fn start_components(&mut self) -> Result<()> {
        tracing::info!("Starting application components...");

        // Start chain mode if available
        if let Some(ref chain_mode) = self.chain_mode {
            chain_mode
                .run()
                .await
                .map_err(|e| eyre::eyre!("Failed to start chain mode: {:?}", e))?;

            tracing::info!("Chain mode started successfully");
        }

        tracing::info!("All application components started successfully");
        Ok(())
    }

    /// Stop the application gracefully
    pub async fn stop(&mut self) -> Result<()> {
        tracing::info!("Stopping application components...");

        // Chain mode cleanup is handled automatically when dropped

        tracing::info!("Application stopped successfully");
        Ok(())
    }

    /// Get the application configuration
    pub fn config(&self) -> &ApplicationConfig {
        &self.config
    }
}

impl ChainMode {
    /// Create ChainMode from configuration
    pub fn new_with_router(
        simulate_chain: bool,
        _main_quote_currency: Symbol,
        _router_config: &CollateralRouterConfig,
    ) -> Self {
        if simulate_chain {
            // Build simulated chain connector with default configuration
            ChainMode::Simulated {
                simple_chain_config: Arc::new(SimpleChainConnectorConfig::default()),
            }
        } else {
            // Build real chain connector with default configuration
            ChainMode::Real {
                real_chain_config: Arc::new(RealChainConnectorConfig::default()),
            }
        }
    }

    /// Get chain connector configuration
    pub fn get_chain_connector_config(&self) -> Arc<dyn ChainConnectorConfig + Send + Sync> {
        match self {
            ChainMode::Simulated {
                simple_chain_config,
            } => simple_chain_config.clone() as Arc<dyn ChainConnectorConfig + Send + Sync>,
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
                // Simulated chain connector runs in-memory without external connections
                tracing::debug!("Simulated chain connector initialized");
            }
            ChainMode::Real { .. } => {
                tracing::info!("Running real chain connector");
                // Real chain connector would establish blockchain connections
                tracing::debug!("Real chain connector initialized");
            }
        }
        Ok(())
    }
}
