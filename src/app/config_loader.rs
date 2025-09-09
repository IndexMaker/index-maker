use eyre::Result;
use std::collections::HashMap;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use symm_core::core::bits::Symbol;

use crate::app::config::{ApplicationConfig, CliOverrides, ConfigBuildError};
use crate::{Cli, SecretsProvider};

/// Configuration loader that handles loading from files and applying CLI overrides
pub struct ConfigLoader {
    config_path: String,
    cli_overrides: CliOverrides,
    secrets: SecretsProvider,
}

impl ConfigLoader {
    /// Default configuration file path
    const DEFAULT_CONFIG_PATH: &'static str = "configs/config.json";

    /// Create a new configuration loader with pre-loaded secrets
    /// This avoids duplicate secrets loading when secrets are already available
    pub fn new_with_secrets(cli: &Cli, secrets: SecretsProvider) -> Result<Self> {
        let config_path = cli
            .config_path
            .as_deref()
            .unwrap_or(Self::DEFAULT_CONFIG_PATH);

        let cli_overrides = Self::extract_cli_overrides(cli)?;

        Ok(Self {
            config_path: String::from(config_path),
            cli_overrides,
            secrets,
        })
    }

    /// Create a new configuration loader (legacy method)
    /// This method loads secrets internally - prefer new_with_secrets() when secrets are already available
    pub fn new(cli: &Cli) -> Result<Self> {
        let config_path = cli
            .config_path
            .as_deref()
            .unwrap_or(Self::DEFAULT_CONFIG_PATH);

        let cli_overrides = Self::extract_cli_overrides(cli)?;
        let secrets = Self::load_secrets()?;

        Ok(Self {
            config_path: String::from(config_path),
            cli_overrides,
            secrets,
        })
    }

    /// Load the complete application configuration
    pub fn load_config(&self) -> Result<ApplicationConfig> {
        tracing::info!(config_path = %self.config_path, "Loading application configuration");

        // Load base configuration from file
        let mut config = self.load_config_from_file()?;

        // Apply CLI overrides
        self.apply_cli_overrides(&mut config)?;

        // Validate configuration
        self.validate_config(&config)?;

        // Apply secrets
        self.apply_secrets(&mut config)?;

        tracing::info!("Application configuration loaded successfully");
        Ok(config)
    }

    /// Load configuration from JSON file
    fn load_config_from_file(&self) -> Result<ApplicationConfig> {
        if !Path::new(&self.config_path).exists() {
            return Err(eyre::eyre!(
                "Failed to load configuration: file not found: {}",
                self.config_path
            ));
        }

        let config_content = fs::read_to_string(&self.config_path)
            .map_err(|e| eyre::eyre!("Failed to read configuration file: {:?}", e))?;

        let config: ApplicationConfig = serde_json::from_str(&config_content)
            .map_err(|e| eyre::eyre!("Failed to parse configuration JSON: {:?}", e))?;

        tracing::debug!(
            config_path = %self.config_path,
            "Configuration loaded from file"
        );

        Ok(config)
    }

    /// Extract CLI overrides from command line arguments
    fn extract_cli_overrides(cli: &Cli) -> Result<CliOverrides> {
        Ok(CliOverrides {
            main_quote_currency: cli.main_quote_currency.clone(),
            simulate_sender: if cli.simulate_sender {
                Some(true)
            } else {
                None
            },
            simulate_chain: if cli.simulate_chain { Some(true) } else { None },
            bind_address: cli.bind_address.clone(),
            log_path: cli.log_path.map(|p| PathBuf::from(String::from(p))),
            term_log_off: if cli.term_log_off { Some(true) } else { None },
            otlp_trace_url: cli.otlp_trace_url.clone(),
            otlp_log_url: cli.otlp_log_url.clone(),
            batch_size: cli.batch_size,
        })
    }

    /// Apply CLI overrides to the configuration
    fn apply_cli_overrides(&self, config: &mut ApplicationConfig) -> Result<()> {
        if let Some(ref currency) = self.cli_overrides.main_quote_currency {
            config.app.main_quote_currency = currency.clone();
            tracing::debug!(currency = %currency, "Applied CLI override for main quote currency");
        }

        if let Some(simulate_sender) = self.cli_overrides.simulate_sender {
            config.app.simulate_sender = simulate_sender;
            tracing::debug!(simulate_sender, "Applied CLI override for simulate sender");
        }

        if let Some(simulate_chain) = self.cli_overrides.simulate_chain {
            config.app.simulate_chain = simulate_chain;
            tracing::debug!(simulate_chain, "Applied CLI override for simulate chain");
        }

        if let Some(ref bind_address) = self.cli_overrides.bind_address {
            config.server.bind_address = bind_address.clone();
            tracing::debug!(bind_address = %bind_address, "Applied CLI override for bind address");
        }

        Ok(())
    }

    /// Load secrets from environment variables
    fn load_secrets() -> Result<SecretsProvider> {
        let required_vars: [&str; 0] = [
            // Add required environment variables here when needed
            // "INDEX_MAKER_API_KEY",
            // "INDEX_MAKER_API_SECRET",
        ];

        // Check for required environment variables
        let missing_vars = required_vars
            .iter()
            .filter(|&&var| env::var(var).is_err())
            .collect::<Vec<_>>();

        if !missing_vars.is_empty() {
            return Err(eyre::eyre!(
                "Failed to load secrets: missing environment variables: {:?}",
                missing_vars
            ));
        }

        // Load optional environment variables
        let mut secrets_map = HashMap::new();

        // Binance credentials
        if let Ok(api_key) = env::var("BINANCE_API_KEY") {
            secrets_map.insert(String::from("binance_api_key"), api_key);
        }
        if let Ok(api_secret) = env::var("BINANCE_API_SECRET") {
            secrets_map.insert(String::from("binance_api_secret"), api_secret);
        }

        // Bitget credentials
        if let Ok(api_key) = env::var("BITGET_API_KEY") {
            secrets_map.insert(String::from("bitget_api_key"), api_key);
        }
        if let Ok(api_secret) = env::var("BITGET_API_SECRET") {
            secrets_map.insert(String::from("bitget_api_secret"), api_secret);
        }

        // Chain credentials
        if let Ok(private_key) = env::var("CHAIN_PRIVATE_KEY") {
            secrets_map.insert(String::from("chain_private_key"), private_key);
        }
        if let Ok(rpc_url) = env::var("CHAIN_RPC_URL") {
            secrets_map.insert(String::from("chain_rpc_url"), rpc_url);
        }

        tracing::debug!(
            secrets_count = secrets_map.len(),
            "Loaded secrets from environment"
        );

        Ok(SecretsProvider::new(secrets_map))
    }

    /// Validate the loaded configuration
    fn validate_config(&self, config: &ApplicationConfig) -> Result<()> {
        // Validate basket manager configuration
        if config.basket_manager.index_files.is_empty() {
            return Err(eyre::eyre!(
                "Failed to validate configuration: basket_manager.index_files cannot be empty"
            ));
        }

        config
            .basket_manager
            .index_files
            .iter()
            .try_for_each(|mapping| {
                if String::from(mapping.symbol.clone()).is_empty() {
                    return Err(eyre::eyre!(
                        "Failed to validate configuration: index symbol cannot be empty"
                    ));
                }

                if !Path::new(&mapping.file_path).exists() {
                    tracing::warn!(
                        symbol = %mapping.symbol,
                        file_path = %mapping.file_path,
                        "Index file does not exist - will be created if needed"
                    );
                }

                Ok(())
            })?;

        tracing::debug!("Configuration validation completed successfully");
        Ok(())
    }

    /// Apply secrets to the configuration
    fn apply_secrets(&self, config: &mut ApplicationConfig) -> Result<()> {
        // Apply Binance credentials if available
        if let Some(api_key) = self.secrets.get_secret("binance_api_key") {
            config.order_sender.api_key = Some(api_key);
        }

        if let Some(api_secret) = self.secrets.get_secret("binance_api_secret") {
            config.order_sender.api_secret = Some(api_secret);
        }

        // Apply chain credentials if available
        if let Some(private_key) = self.secrets.get_secret("chain_private_key") {
            config.chain.private_key = Some(private_key);
        }

        if let Some(rpc_url) = self.secrets.get_secret("chain_rpc_url") {
            config.chain.rpc_url = Some(rpc_url);
        }

        tracing::debug!("Secrets applied to configuration");
        Ok(())
    }
}
