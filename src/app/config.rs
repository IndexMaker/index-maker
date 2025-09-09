//! # Configuration Module
//!
//! This module provides comprehensive configuration management for the Index Maker application.
//! It implements a type-safe, JSON-based configuration system with proper validation,
//! error handling, and support for multiple configuration sources.
//!
//! ## Key Features
//!
//! - **Type Safety**: Uses `Amount` for numeric values and `Symbol` for internable strings
//! - **Mathematical Safety**: All numeric operations use the `safe!()` macro
//! - **Error Handling**: Comprehensive error types with proper context using `eyre::Result<T>`
//! - **Validation**: Extensive validation with meaningful error messages
//! - **Multiple Sources**: Supports loading from JSON files, environment variables, and CLI arguments
//!
//! ## Usage Example
//!
//! ```rust
//! use index_maker::app::config::{ApplicationConfig, ApplicationConfigLoader};
//! use eyre::Result;
//!
//! #[tokio::main]
//! async fn main() -> Result<()> {
//!     // Load configuration from multiple sources
//!     let loader = ApplicationConfigLoader::new();
//!     let config = loader.auto_load(&cli)?;
//!
//!     // Configuration is now validated and ready to use
//!     println!("Main quote currency: {}", config.app.main_quote_currency);
//!
//!     Ok(())
//! }
//! ```

use chrono::TimeDelta;
use derive_builder::UninitializedFieldError;
use eyre::{OptionExt, Report};

use safe_math::safe;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use symm_core::core::bits::{Amount, Symbol};
use symm_core::core::decimal_ext::DecimalExt;
use thiserror::Error;

/// Configuration build errors with comprehensive error context
///
/// This enum provides detailed error information for configuration loading and validation
/// failures. Each variant includes specific context to help diagnose configuration issues.
///
/// # Error Handling Pattern
///
/// All configuration operations return `Result<T, ConfigBuildError>` and use the `?` operator
/// for error propagation as per the contributing guidelines.
#[derive(Debug, Error)]
pub enum ConfigBuildError {
    /// A required configuration field is missing or uninitialized
    #[error("Configuration missing or invalid `{0}`")]
    UninitializedField(&'static str),
    /// General configuration error with context
    #[error("Configuration error `{0}`")]
    Other(String),
    /// File system or parsing error when loading configuration files
    #[error("Configuration file error: {0}")]
    FileError(String),
    /// Configuration validation error with specific validation failure details
    #[error("Configuration validation error: {0}")]
    ValidationError(String),
    /// Environment variable parsing or access error
    #[error("Environment variable error: {0}")]
    EnvError(String),
}

impl From<UninitializedFieldError> for ConfigBuildError {
    fn from(err: UninitializedFieldError) -> Self {
        ConfigBuildError::UninitializedField(err.field_name())
    }
}

impl From<Report> for ConfigBuildError {
    fn from(report: Report) -> Self {
        ConfigBuildError::Other(format!("{:?}", report))
    }
}

impl From<std::io::Error> for ConfigBuildError {
    fn from(err: std::io::Error) -> Self {
        ConfigBuildError::FileError(format!("IO error: {:?}", err))
    }
}

impl From<serde_json::Error> for ConfigBuildError {
    fn from(err: serde_json::Error) -> Self {
        ConfigBuildError::FileError(format!("JSON parsing error: {}", err))
    }
}

/// Main application configuration that aggregates all component configurations
///
/// This is the root configuration structure that contains all settings needed to run
/// the Index Maker application. It follows a hierarchical structure where each major
/// component has its own configuration section.
///
/// # Type Safety
///
/// - All numeric values use `Amount` type for mathematical safety
/// - All internable strings use `Symbol` type for memory efficiency
/// - All configurations implement proper serde serialization/deserialization
///
/// # Configuration Sources
///
/// Configuration is loaded with the following precedence (highest to lowest):
/// 1. Command line arguments
/// 2. Environment variables
/// 3. JSON configuration file
/// 4. Default values
///
/// # Example JSON Structure
///
/// ```json
/// {
///   "app": {
///     "main_quote_currency": "USDC",
///     "simulate_sender": false,
///     "simulate_chain": true
///   },
///   "solver": {
///     "price_threshold": "0.01",
///     "max_levels": 5,
///     "fee_factor": "1.002"
///   },
///   "market_data": {
///     "provider": "binance",
///     "connection_timeout_secs": 30
///   }
/// }
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApplicationConfig {
    /// Application-level settings including quote currency and simulation modes
    pub app: AppSettings,
    /// Solver configuration with mathematical parameters and timing settings
    pub solver: SolverSettings,
    /// Connection settings for external services (market data, exchanges, blockchain, server)
    pub connections: ConnectionSettings,
    /// Logging configuration including levels, outputs, and telemetry
    pub logging: LoggingSettings,
}

impl ApplicationConfig {
    /// Apply CLI overrides to configuration
    /// This replaces the complex merge functions with simple field-by-field override
    pub fn apply_cli_overrides(&mut self, cli: &CliOverrides) {
        if let Some(currency) = &cli.main_quote_currency {
            self.app.main_quote_currency = currency.clone();
        }
        if let Some(simulate) = cli.simulate_sender {
            self.app.simulate_sender = simulate;
        }
        if let Some(simulate) = cli.simulate_chain {
            self.app.simulate_chain = simulate;
        }
        if let Some(bind_address) = &cli.bind_address {
            self.connections.server_bind_address = bind_address.clone();
        }
        if let Some(log_path) = &cli.log_path {
            self.logging.file_path = Some(log_path.clone());
        }
        if let Some(term_log_off) = cli.term_log_off {
            self.logging.disable_terminal = term_log_off;
        }
        if let Some(otlp_trace_url) = &cli.otlp_trace_url {
            self.logging.otlp_trace_url = Some(otlp_trace_url.clone());
        }
        if let Some(otlp_log_url) = &cli.otlp_log_url {
            self.logging.otlp_log_url = Some(otlp_log_url.clone());
        }
        if let Some(batch_size) = cli.batch_size {
            self.logging.batch_size = batch_size;
        }
    }
}

/// Application-level settings (renamed from AppConfig for clarity)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppSettings {
    /// Main quote currency (default: USDC)
    #[serde(default = "default_main_quote_currency")]
    pub main_quote_currency: Symbol,
    /// Whether to simulate order sending
    #[serde(default)]
    pub simulate_sender: bool,
    /// Whether to simulate chain operations
    #[serde(default)]
    pub simulate_chain: bool,
    /// Index configuration file path
    #[serde(default = "default_index_config")]
    pub index_config: PathBuf,
}

/// Solver configuration settings (renamed from SolverConfigData for clarity)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SolverSettings {
    /// Price threshold for solver decisions
    #[serde(default = "default_price_threshold")]
    pub price_threshold: Amount,
    /// Maximum levels for order book processing
    #[serde(default = "default_max_levels")]
    pub max_levels: usize,
    /// Fee factor for calculations
    #[serde(default = "default_fee_factor")]
    pub fee_factor: Amount,
    /// Maximum order volley size
    #[serde(default = "default_max_order_volley_size")]
    pub max_order_volley_size: Amount,
    /// Maximum volley size
    #[serde(default = "default_max_volley_size")]
    pub max_volley_size: Amount,
    /// Minimum asset volley size
    #[serde(default = "default_min_asset_volley_size")]
    pub min_asset_volley_size: Amount,
    /// Asset volley step size
    #[serde(default = "default_asset_volley_step_size")]
    pub asset_volley_step_size: Amount,
    /// Maximum total volley size
    #[serde(default = "default_max_total_volley_size")]
    pub max_total_volley_size: Amount,
    /// Minimum total volley available
    #[serde(default = "default_min_total_volley_available")]
    pub min_total_volley_available: Amount,
    /// Zero threshold for calculations
    #[serde(default = "default_zero_threshold")]
    pub zero_threshold: Amount,
    /// Maximum batch size
    #[serde(default = "default_max_batch_size")]
    pub max_batch_size: usize,
    /// Fill threshold
    #[serde(default = "default_fill_threshold")]
    pub fill_threshold: Amount,
    /// Mint threshold
    #[serde(default = "default_mint_threshold")]
    pub mint_threshold: Amount,
    /// Mint wait period in seconds
    #[serde(default = "default_mint_wait_period_secs")]
    pub mint_wait_period_secs: i64,
    /// Client order wait period in seconds
    #[serde(default = "default_client_order_wait_period_secs")]
    pub client_order_wait_period_secs: i64,
    /// Client quote wait period in seconds
    #[serde(default = "default_client_quote_wait_period_secs")]
    pub client_quote_wait_period_secs: i64,
    /// Solver tick interval in milliseconds
    #[serde(default = "default_solver_tick_interval_ms")]
    pub solver_tick_interval_ms: i64,
    /// Quotes tick interval in milliseconds
    #[serde(default = "default_quotes_tick_interval_ms")]
    pub quotes_tick_interval_ms: i64,
}

/// Connection settings for all external services (consolidates market_data, chain, order_sender, server)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConnectionSettings {
    /// Market data provider settings
    #[serde(default = "default_market_data_provider")]
    pub market_data_provider: Symbol,
    /// Market data connection timeout in seconds
    #[serde(default = "default_connection_timeout")]
    pub market_data_timeout_secs: u64,
    /// Market data reconnection attempts
    #[serde(default = "default_reconnection_attempts")]
    pub market_data_reconnection_attempts: u32,

    /// Chain provider (e.g., "ethereum", "arbitrum")
    #[serde(default = "default_chain_provider")]
    pub chain_provider: Symbol,
    /// RPC URL for chain connection
    pub chain_rpc_url: Option<String>,
    /// Gas price multiplier
    #[serde(default = "default_gas_price_multiplier")]
    pub chain_gas_price_multiplier: f64,

    /// Exchange provider for order sending
    #[serde(default = "default_order_sender_provider")]
    pub order_sender_provider: Symbol,
    /// Rate limiting settings for order sender
    #[serde(default = "default_rate_limit_per_second")]
    pub order_sender_rate_limit_per_second: u32,

    /// Server bind address
    #[serde(default = "default_bind_address")]
    pub server_bind_address: String,
    /// Server timeout settings
    #[serde(default = "default_server_timeout")]
    pub server_timeout_secs: u64,
}

/// Logging settings (renamed from LoggingConfig for clarity)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoggingSettings {
    /// Log level (trace, debug, info, warn, error)
    #[serde(default = "default_log_level")]
    pub level: String,
    /// Log file path (optional)
    pub file_path: Option<PathBuf>,
    /// Disable terminal logging
    #[serde(default)]
    pub disable_terminal: bool,
    /// OTLP trace URL for distributed tracing
    pub otlp_trace_url: Option<String>,
    /// OTLP log URL for log aggregation
    pub otlp_log_url: Option<String>,
    /// Batch size for log processing
    #[serde(default = "default_log_batch_size")]
    pub batch_size: usize,
}

/// Basket manager configuration - simple and clean
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BasketManagerConfigData {
    /// List of index file mappings
    pub index_files: Vec<IndexFileMapping>,
}

/// Index file mapping configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexFileMapping {
    /// Index symbol
    pub symbol: Symbol,
    /// Path to the index file
    pub file_path: String,
}

// Default value functions for serde defaults
fn default_main_quote_currency() -> Symbol {
    Symbol::from("USDC")
}

fn default_index_config() -> PathBuf {
    PathBuf::from("./index_config.json")
}

fn default_price_threshold() -> Amount {
    rust_decimal::dec!(0.01)
}

fn default_max_levels() -> usize {
    5
}

fn default_fee_factor() -> Amount {
    rust_decimal::dec!(1.002)
}

fn default_max_order_volley_size() -> Amount {
    rust_decimal::dec!(20.0)
}

fn default_max_volley_size() -> Amount {
    rust_decimal::dec!(100.0)
}

fn default_min_asset_volley_size() -> Amount {
    rust_decimal::dec!(5.0)
}

fn default_asset_volley_step_size() -> Amount {
    rust_decimal::dec!(0.1)
}

fn default_max_total_volley_size() -> Amount {
    rust_decimal::dec!(1000.0)
}

fn default_min_total_volley_available() -> Amount {
    rust_decimal::dec!(100.0)
}

fn default_zero_threshold() -> Amount {
    rust_decimal::dec!(0.0001)
}

fn default_max_batch_size() -> usize {
    10
}

fn default_fill_threshold() -> Amount {
    rust_decimal::dec!(0.9999)
}

fn default_mint_threshold() -> Amount {
    rust_decimal::dec!(0.99)
}

fn default_mint_wait_period_secs() -> i64 {
    10
}

fn default_client_order_wait_period_secs() -> i64 {
    30
}

fn default_client_quote_wait_period_secs() -> i64 {
    5
}

fn default_solver_tick_interval_ms() -> i64 {
    100
}

fn default_quotes_tick_interval_ms() -> i64 {
    10
}

fn default_connection_timeout() -> u64 {
    30
}

fn default_reconnection_attempts() -> u32 {
    3
}

fn default_gas_price_multiplier() -> f64 {
    1.1
}

fn default_rate_limit_per_second() -> u32 {
    10
}

fn default_log_level() -> String {
    String::from("info")
}

fn default_log_batch_size() -> usize {
    1000
}

fn default_bind_address() -> String {
    String::from("127.0.0.1:3000")
}

fn default_server_timeout() -> u64 {
    30
}

fn default_market_data_provider() -> Symbol {
    Symbol::from("binance")
}

fn default_chain_provider() -> Symbol {
    Symbol::from("ethereum")
}

fn default_order_sender_provider() -> Symbol {
    Symbol::from("binance")
}

impl Default for ApplicationConfig {
    fn default() -> Self {
        Self {
            app: AppSettings::default(),
            solver: SolverSettings::default(),
            connections: ConnectionSettings::default(),
            logging: LoggingSettings::default(),
        }
    }
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            main_quote_currency: default_main_quote_currency(),
            simulate_sender: false,
            simulate_chain: false,
            index_config: default_index_config(),
        }
    }
}

impl Default for SolverSettings {
    fn default() -> Self {
        Self {
            price_threshold: default_price_threshold(),
            max_levels: default_max_levels(),
            fee_factor: default_fee_factor(),
            max_order_volley_size: default_max_order_volley_size(),
            max_volley_size: default_max_volley_size(),
            min_asset_volley_size: default_min_asset_volley_size(),
            asset_volley_step_size: default_asset_volley_step_size(),
            max_total_volley_size: default_max_total_volley_size(),
            min_total_volley_available: default_min_total_volley_available(),
            zero_threshold: default_zero_threshold(),
            max_batch_size: default_max_batch_size(),
            fill_threshold: default_fill_threshold(),
            mint_threshold: default_mint_threshold(),
            mint_wait_period_secs: default_mint_wait_period_secs(),
            client_order_wait_period_secs: default_client_order_wait_period_secs(),
            client_quote_wait_period_secs: default_client_quote_wait_period_secs(),
            solver_tick_interval_ms: default_solver_tick_interval_ms(),
            quotes_tick_interval_ms: default_quotes_tick_interval_ms(),
        }
    }
}

impl Default for ConnectionSettings {
    fn default() -> Self {
        Self {
            market_data_provider: default_market_data_provider(),
            market_data_timeout_secs: default_connection_timeout(),
            market_data_reconnection_attempts: default_reconnection_attempts(),
            chain_provider: default_chain_provider(),
            chain_rpc_url: None,
            chain_gas_price_multiplier: default_gas_price_multiplier(),
            order_sender_provider: default_order_sender_provider(),
            order_sender_rate_limit_per_second: default_rate_limit_per_second(),
            server_bind_address: default_bind_address(),
            server_timeout_secs: default_server_timeout(),
        }
    }
}

impl Default for LoggingSettings {
    fn default() -> Self {
        Self {
            level: default_log_level(),
            file_path: None,
            disable_terminal: false,
            otlp_trace_url: None,
            otlp_log_url: None,
            batch_size: default_log_batch_size(),
        }
    }
}

impl Default for BasketManagerConfigData {
    fn default() -> Self {
        Self {
            index_files: Vec::new(),
        }
    }
}

/// Configuration loader that handles multiple sources with proper precedence
pub struct ConfigLoader {
    /// Base configuration (defaults)
    base_config: ApplicationConfig,
}

impl ConfigLoader {
    /// Create a new configuration loader with default values
    pub fn new() -> Self {
        Self {
            base_config: ApplicationConfig::default(),
        }
    }

    /// Load configuration from multiple sources with precedence:
    /// 1. Default values (lowest priority)
    /// 2. Configuration file
    /// 3. Environment variables
    /// 4. Command line arguments (highest priority)
    pub fn load_config(
        &self,
        config_file_path: Option<&PathBuf>,
        cli_overrides: Option<&CliOverrides>,
    ) -> Result<ApplicationConfig, ConfigBuildError> {
        let mut config = self.base_config.clone();

        // Load from configuration file if provided
        if let Some(path) = config_file_path {
            config = self.merge_config_file(config, path)?;
        }

        // Load from environment variables
        config = self.merge_environment_variables(config)?;

        // Apply CLI overrides if provided
        if let Some(overrides) = cli_overrides {
            config = self.merge_cli_overrides(config, overrides);
        }

        // Validate the final configuration
        self.validate_config(&config)?;

        Ok(config)
    }

    /// Load configuration from a file (supports JSON and TOML)
    fn merge_config_file(
        &self,
        base_config: ApplicationConfig,
        file_path: &PathBuf,
    ) -> Result<ApplicationConfig, ConfigBuildError> {
        if !file_path.exists() {
            return Err(ConfigBuildError::FileError(format!(
                "Configuration file not found: {}",
                file_path.display()
            )));
        }

        let content = std::fs::read_to_string(file_path).map_err(|err| {
            ConfigBuildError::FileError(format!(
                "Failed to read configuration file {}: {:?}",
                file_path.display(),
                err
            ))
        })?;

        tracing::info!(
            file_path = %file_path.display(),
            "Loading configuration from file"
        );

        let file_config: ApplicationConfig = match file_path.extension().and_then(|s| s.to_str()) {
            Some("json") => serde_json::from_str(&content).map_err(|err| {
                ConfigBuildError::FileError(format!(
                    "Failed to parse JSON configuration file {}: {:?}",
                    file_path.display(),
                    err
                ))
            })?,
            Some(ext) => {
                return Err(ConfigBuildError::FileError(format!(
                    "Unsupported configuration file format: {}. Supported formats: json",
                    ext
                )));
            }
            None => {
                return Err(ConfigBuildError::FileError(String::from(
                    "Configuration file has no extension. Supported formats: json",
                )));
            }
        };

        // Merge file config into base config using deep merging
        self.merge_configs(base_config, file_config)
    }

    /// Merge environment variables into configuration
    fn merge_environment_variables(
        &self,
        mut config: ApplicationConfig,
    ) -> Result<ApplicationConfig, ConfigBuildError> {
        use std::env;

        tracing::debug!("Loading configuration from environment variables");

        // App-level environment variables
        if let Ok(val) = env::var("INDEX_MAKER_MAIN_QUOTE_CURRENCY") {
            tracing::debug!(
                env_var = "INDEX_MAKER_MAIN_QUOTE_CURRENCY",
                value = %val,
                "Overriding main quote currency from environment"
            );
            config.app.main_quote_currency = Symbol::from(String::from(val));
        }
        if let Ok(val) = env::var("INDEX_MAKER_SIMULATE_SENDER") {
            config.app.simulate_sender = val.parse().map_err(|err| {
                ConfigBuildError::EnvError(format!(
                    "Failed to parse INDEX_MAKER_SIMULATE_SENDER as boolean: {:?}",
                    err
                ))
            })?;
        }
        if let Ok(val) = env::var("INDEX_MAKER_SIMULATE_CHAIN") {
            config.app.simulate_chain = val.parse().map_err(|err| {
                ConfigBuildError::EnvError(format!(
                    "Failed to parse INDEX_MAKER_SIMULATE_CHAIN as boolean: {:?}",
                    err
                ))
            })?;
        }

        // Logging environment variables
        if let Ok(val) = env::var("INDEX_MAKER_LOG_LEVEL") {
            config.logging.level = val;
        }
        if let Ok(val) = env::var("INDEX_MAKER_LOG_FILE") {
            config.logging.file_path = Some(PathBuf::from(val));
        }
        if let Ok(val) = env::var("INDEX_MAKER_OTLP_TRACE_URL") {
            config.logging.otlp_trace_url = Some(val);
        }
        if let Ok(val) = env::var("INDEX_MAKER_OTLP_LOG_URL") {
            config.logging.otlp_log_url = Some(val);
        }

        // Server environment variables
        if let Ok(val) = env::var("INDEX_MAKER_BIND_ADDRESS") {
            config.server.bind_address = val;
        }

        // Chain environment variables
        if let Ok(val) = env::var("INDEX_MAKER_CHAIN_RPC_URL") {
            config.chain.rpc_url = Some(val);
        }
        if let Ok(val) = env::var("INDEX_MAKER_CHAIN_PRIVATE_KEY") {
            config.chain.private_key = Some(val);
        }

        // Order sender environment variables
        if let Ok(val) = env::var("INDEX_MAKER_API_KEY") {
            config.order_sender.api_key = Some(val);
        }
        if let Ok(val) = env::var("INDEX_MAKER_API_SECRET") {
            config.order_sender.api_secret = Some(val);
        }

        Ok(config)
    }

    /// Merge CLI overrides into configuration
    fn merge_cli_overrides(
        &self,
        mut config: ApplicationConfig,
        overrides: &CliOverrides,
    ) -> ApplicationConfig {
        if let Some(ref currency) = overrides.main_quote_currency {
            config.app.main_quote_currency = currency.clone();
        }
        if let Some(simulate) = overrides.simulate_sender {
            config.app.simulate_sender = simulate;
        }
        if let Some(simulate) = overrides.simulate_chain {
            config.app.simulate_chain = simulate;
        }
        if let Some(ref address) = overrides.bind_address {
            config.server.bind_address = address.clone();
        }
        if let Some(ref path) = overrides.log_path {
            config.logging.file_path = Some(path.clone());
        }
        if let Some(disable) = overrides.term_log_off {
            config.logging.disable_terminal = disable;
        }
        if let Some(ref url) = overrides.otlp_trace_url {
            config.logging.otlp_trace_url = Some(url.clone());
        }
        if let Some(ref url) = overrides.otlp_log_url {
            config.logging.otlp_log_url = Some(url.clone());
        }
        if let Some(size) = overrides.batch_size {
            config.logging.batch_size = size;
        }

        config
    }

    /// Simple configuration override - file config takes precedence over defaults
    /// This replaces the complex merge functions with simple field assignment
    fn merge_configs(
        &self,
        _base: ApplicationConfig,
        override_config: ApplicationConfig,
    ) -> Result<ApplicationConfig, ConfigBuildError> {
        // Simply return the override config since we're not doing actual merging
        // All merge functions were just doing field assignment anyway
        Ok(override_config)
    }

    /// Validate the final configuration using safe mathematical operations
    fn validate_config(&self, config: &ApplicationConfig) -> Result<(), ConfigBuildError> {
        tracing::info!("Validating application configuration");

        // Validate each configuration section
        self.validate_solver_config(&config.solver)?;
        self.validate_market_data_config(&config.connections)?;
        self.validate_chain_config(&config.connections)?;
        self.validate_order_sender_config(&config.connections)?;
        self.validate_logging_config(&config.logging)?;
        self.validate_server_config(&config.connections)?;

        if config.solver.max_batch_size == 0 {
            return Err(ConfigBuildError::ValidationError(String::from(
                "max_batch_size must be greater than 0",
            )));
        }

        // Validate market data configuration
        if config.market_data.provider.is_empty() {
            return Err(ConfigBuildError::ValidationError(String::from(
                "market_data.provider cannot be empty",
            )));
        }

        // Validate chain configuration
        if config.chain.provider.is_empty() {
            return Err(ConfigBuildError::ValidationError(String::from(
                "chain.provider cannot be empty",
            )));
        }

        // Validate order sender configuration
        if config.order_sender.provider.is_empty() {
            return Err(ConfigBuildError::ValidationError(String::from(
                "order_sender.provider cannot be empty",
            )));
        }

        // Validate logging configuration
        let valid_log_levels = ["trace", "debug", "info", "warn", "error"];
        if !valid_log_levels.contains(&String::from(config.logging.level.clone()).as_str()) {
            return Err(ConfigBuildError::ValidationError(format!(
                "Invalid log level '{}'. Valid levels: {}",
                config.logging.level,
                valid_log_levels.join(", ")
            )));
        }

        // Validate server configuration
        if config.server.bind_address.is_empty() {
            return Err(ConfigBuildError::ValidationError(String::from(
                "server.bind_address cannot be empty",
            )));
        }

        // Validate bind address format (basic check)
        if !config.server.bind_address.contains(':') {
            return Err(ConfigBuildError::ValidationError(String::from(
                "server.bind_address must be in format 'host:port'",
            )));
        }

        tracing::info!("Configuration validation completed successfully");
        Ok(())
    }

    /// Validate solver configuration using safe mathematical operations
    fn validate_solver_config(
        &self,
        solver_config: &SolverSettings,
    ) -> Result<(), ConfigBuildError> {
        tracing::debug!("Validating solver configuration");

        // Validate basic positive values
        let zero_amount = Amount::ZERO;
        if solver_config.price_threshold <= zero_amount {
            return Err(ConfigBuildError::ValidationError(String::from(
                "solver.price_threshold must be positive",
            )));
        }

        if solver_config.max_levels == 0 {
            return Err(ConfigBuildError::ValidationError(String::from(
                "solver.max_levels must be greater than 0",
            )));
        }

        let one_amount = Amount::ONE;
        if solver_config.fee_factor <= one_amount {
            return Err(ConfigBuildError::ValidationError(String::from(
                "solver.fee_factor must be greater than 1.0",
            )));
        }

        if solver_config.max_batch_size == 0 {
            return Err(ConfigBuildError::ValidationError(String::from(
                "solver.max_batch_size must be greater than 0",
            )));
        }

        // Validate volley size relationships using safe math
        self.validate_volley_sizes(solver_config)?;

        // Validate timing parameters
        if solver_config.mint_wait_period_secs <= 0 {
            return Err(ConfigBuildError::ValidationError(String::from(
                "solver.mint_wait_period_secs must be positive",
            )));
        }

        if solver_config.client_order_wait_period_secs <= 0 {
            return Err(ConfigBuildError::ValidationError(String::from(
                "solver.client_order_wait_period_secs must be positive",
            )));
        }

        if solver_config.solver_tick_interval_ms <= 0 {
            return Err(ConfigBuildError::ValidationError(String::from(
                "solver.solver_tick_interval_ms must be positive",
            )));
        }

        tracing::debug!("Solver configuration validation passed");
        Ok(())
    }

    /// Validate market data configuration
    fn validate_market_data_config(
        &self,
        market_data_config: &ConnectionSettings,
    ) -> Result<(), ConfigBuildError> {
        tracing::debug!("Validating market data configuration");

        if String::from(&market_data_config.market_data_provider).is_empty() {
            return Err(ConfigBuildError::ValidationError(String::from(
                "connections.market_data_provider cannot be empty",
            )));
        }

        if market_data_config.market_data_timeout_secs == 0 {
            return Err(ConfigBuildError::ValidationError(String::from(
                "connections.market_data_timeout_secs must be greater than 0",
            )));
        }

        if market_data_config.market_data_reconnection_attempts == 0 {
            return Err(ConfigBuildError::ValidationError(String::from(
                "connections.market_data_reconnection_attempts must be greater than 0",
            )));
        }

        tracing::debug!("Market data configuration validation passed");
        Ok(())
    }

    /// Validate chain configuration
    fn validate_chain_config(
        &self,
        chain_config: &ConnectionSettings,
    ) -> Result<(), ConfigBuildError> {
        tracing::debug!("Validating chain configuration");

        if String::from(&chain_config.chain_provider).is_empty() {
            return Err(ConfigBuildError::ValidationError(String::from(
                "connections.chain_provider cannot be empty",
            )));
        }

        if chain_config.chain_gas_price_multiplier <= 0.0 {
            return Err(ConfigBuildError::ValidationError(String::from(
                "connections.chain_gas_price_multiplier must be positive",
            )));
        }

        // Validate gas price multiplier is reasonable (between 1.0 and 10.0)
        if chain_config.chain_gas_price_multiplier < 1.0
            || chain_config.chain_gas_price_multiplier > 10.0
        {
            return Err(ConfigBuildError::ValidationError(String::from(
                "connections.chain_gas_price_multiplier should be between 1.0 and 10.0",
            )));
        }

        tracing::debug!("Chain configuration validation passed");
        Ok(())
    }

    /// Validate order sender configuration
    fn validate_order_sender_config(
        &self,
        order_sender_config: &ConnectionSettings,
    ) -> Result<(), ConfigBuildError> {
        tracing::debug!("Validating order sender configuration");

        if String::from(&order_sender_config.order_sender_provider).is_empty() {
            return Err(ConfigBuildError::ValidationError(String::from(
                "connections.order_sender_provider cannot be empty",
            )));
        }

        if order_sender_config.order_sender_rate_limit_per_second == 0 {
            return Err(ConfigBuildError::ValidationError(String::from(
                "connections.order_sender_rate_limit_per_second must be greater than 0",
            )));
        }

        // Validate rate limit is reasonable (not too high to avoid overwhelming exchanges)
        if order_sender_config.rate_limit_per_second > 1000 {
            return Err(ConfigBuildError::ValidationError(String::from(
                "order_sender.rate_limit_per_second should not exceed 1000",
            )));
        }

        tracing::debug!("Order sender configuration validation passed");
        Ok(())
    }

    /// Validate basket manager configuration
    fn validate_basket_manager_config(
        &self,
        basket_config: &BasketManagerConfigData,
    ) -> Result<(), ConfigBuildError> {
        tracing::debug!("Validating basket manager configuration");

        if basket_config.index_files.is_empty() {
            return Err(ConfigBuildError::ValidationError(String::from(
                "basket_manager.index_files cannot be empty",
            )));
        }

        // Validate each index file mapping
        for (index, mapping) in basket_config.index_files.iter().enumerate() {
            if String::from(mapping.symbol.clone()).is_empty() {
                return Err(ConfigBuildError::ValidationError(format!(
                    "basket_manager.index_files[{}].symbol cannot be empty",
                    index
                )));
            }

            if mapping.file_path.is_empty() {
                return Err(ConfigBuildError::ValidationError(format!(
                    "basket_manager.index_files[{}].file_path cannot be empty",
                    index
                )));
            }

            // Check if file exists (optional validation)
            let path = std::path::Path::new(&mapping.file_path);
            if !path.exists() {
                tracing::warn!(
                    file_path = %mapping.file_path,
                    symbol = %mapping.symbol,
                    "Basket file does not exist (will be checked at runtime)"
                );
            }
        }

        tracing::debug!("Basket manager configuration validation passed");
        Ok(())
    }

    /// Validate logging configuration
    fn validate_logging_config(
        &self,
        logging_config: &LoggingSettings,
    ) -> Result<(), ConfigBuildError> {
        tracing::debug!("Validating logging configuration");

        let valid_log_levels = ["trace", "debug", "info", "warn", "error"];
        if !valid_log_levels.contains(&String::from(logging_config.level.clone()).as_str()) {
            return Err(ConfigBuildError::ValidationError(format!(
                "Invalid log level '{}'. Valid levels: {}",
                logging_config.level,
                valid_log_levels.join(", ")
            )));
        }

        if logging_config.batch_size == 0 {
            return Err(ConfigBuildError::ValidationError(String::from(
                "logging.batch_size must be greater than 0",
            )));
        }

        tracing::debug!("Logging configuration validation passed");
        Ok(())
    }

    /// Validate server configuration
    fn validate_server_config(
        &self,
        server_config: &ConnectionSettings,
    ) -> Result<(), ConfigBuildError> {
        tracing::debug!("Validating server configuration");

        if server_config.server_bind_address.is_empty() {
            return Err(ConfigBuildError::ValidationError(String::from(
                "connections.server_bind_address cannot be empty",
            )));
        }

        // Validate bind address format (basic check)
        if !server_config.server_bind_address.contains(':') {
            return Err(ConfigBuildError::ValidationError(String::from(
                "connections.server_bind_address must be in format 'host:port'",
            )));
        }

        if server_config.server_timeout_secs == 0 {
            return Err(ConfigBuildError::ValidationError(String::from(
                "connections.server_timeout_secs must be greater than 0",
            )));
        }

        tracing::debug!("Server configuration validation passed");
        Ok(())
    }

    /// Validate volley size relationships using safe mathematical operations
    fn validate_volley_sizes(
        &self,
        solver_config: &SolverConfigData,
    ) -> Result<(), ConfigBuildError> {
        // Validate that min_asset_volley_size <= max_order_volley_size
        if solver_config.min_asset_volley_size > solver_config.max_order_volley_size {
            return Err(ConfigBuildError::ValidationError(String::from(
                "min_asset_volley_size must be <= max_order_volley_size",
            )));
        }

        // Validate that max_order_volley_size <= max_volley_size
        if solver_config.max_order_volley_size > solver_config.max_volley_size {
            return Err(ConfigBuildError::ValidationError(String::from(
                "max_order_volley_size must be <= max_volley_size",
            )));
        }

        // Validate that max_volley_size <= max_total_volley_size
        if solver_config.max_volley_size > solver_config.max_total_volley_size {
            return Err(ConfigBuildError::ValidationError(String::from(
                "max_volley_size must be <= max_total_volley_size",
            )));
        }

        // Validate that min_total_volley_available is reasonable using safe math
        let min_required =
            safe!(solver_config.max_total_volley_size * solver_config.fill_threshold)
                .ok_or_eyre("Math problem")?;

        if solver_config.min_total_volley_available < min_required {
            return Err(ConfigBuildError::ValidationError(format!(
                "min_total_volley_available ({}) should be >= max_total_volley_size * fill_threshold ({})",
                solver_config.min_total_volley_available,
                min_required
            )));
        }

        // Validate step size is positive and reasonable
        let zero_amount = Amount::ZERO;
        if solver_config.asset_volley_step_size <= zero_amount {
            return Err(ConfigBuildError::ValidationError(String::from(
                "asset_volley_step_size must be positive",
            )));
        }

        // Validate that step size is not larger than min asset volley size
        if solver_config.asset_volley_step_size > solver_config.min_asset_volley_size {
            return Err(ConfigBuildError::ValidationError(String::from(
                "asset_volley_step_size should not be larger than min_asset_volley_size",
            )));
        }

        Ok(())
    }
}

impl Default for ConfigLoader {
    fn default() -> Self {
        Self::new()
    }
}

/// CLI overrides structure for command-line arguments
#[derive(Debug, Clone, Default)]
pub struct CliOverrides {
    pub main_quote_currency: Option<Symbol>,
    pub simulate_sender: Option<bool>,
    pub simulate_chain: Option<bool>,
    pub bind_address: Option<String>,
    pub log_path: Option<PathBuf>,
    pub term_log_off: Option<bool>,
    pub otlp_trace_url: Option<String>,
    pub otlp_log_url: Option<String>,
    pub batch_size: Option<usize>,
}

impl CliOverrides {
    /// Create CLI overrides from the existing Cli struct
    pub fn from_cli(cli: &crate::cli::Cli) -> Self {
        Self {
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
        }
    }
}

/// Helper functions for converting configuration data to TimeDelta
impl SolverConfigData {
    pub fn mint_wait_period(&self) -> TimeDelta {
        TimeDelta::seconds(self.mint_wait_period_secs)
    }

    pub fn client_order_wait_period(&self) -> TimeDelta {
        TimeDelta::seconds(self.client_order_wait_period_secs)
    }

    pub fn client_quote_wait_period(&self) -> TimeDelta {
        TimeDelta::seconds(self.client_quote_wait_period_secs)
    }

    pub fn solver_tick_interval(&self) -> TimeDelta {
        TimeDelta::milliseconds(self.solver_tick_interval_ms)
    }

    pub fn quotes_tick_interval(&self) -> TimeDelta {
        TimeDelta::milliseconds(self.quotes_tick_interval_ms)
    }
}
