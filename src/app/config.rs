use derive_builder::UninitializedFieldError;
use eyre::Report;
use thiserror::Error;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use chrono::TimeDelta;
use rust_decimal::Decimal;
use symm_core::core::bits::{Amount, Symbol};

#[derive(Debug, Error)]
pub enum ConfigBuildError {
    #[error("Configuration missing or invalid `{0}`")]
    UninitializedField(&'static str),
    #[error("Configuration error `{0}`")]
    Other(String),
    #[error("Configuration file error: {0}")]
    FileError(String),
    #[error("Configuration validation error: {0}")]
    ValidationError(String),
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
        ConfigBuildError::FileError(err.to_string())
    }
}

impl From<serde_json::Error> for ConfigBuildError {
    fn from(err: serde_json::Error) -> Self {
        ConfigBuildError::FileError(format!("JSON parsing error: {}", err))
    }
}

impl From<toml::de::Error> for ConfigBuildError {
    fn from(err: toml::de::Error) -> Self {
        ConfigBuildError::FileError(format!("TOML parsing error: {}", err))
    }
}

/// Main application configuration that aggregates all component configurations
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApplicationConfig {
    /// Application-level settings
    pub app: AppConfig,
    /// Solver configuration
    pub solver: SolverConfigData,
    /// Market data configuration
    pub market_data: MarketDataConfigData,
    /// Chain connector configuration
    pub chain: ChainConfigData,
    /// Order sender configuration
    pub order_sender: OrderSenderConfigData,
    /// Logging configuration
    pub logging: LoggingConfig,
    /// Server configuration
    pub server: ServerConfigData,
}

/// Application-level configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    /// Main quote currency (default: USDC)
    #[serde(default = "default_main_quote_currency")]
    pub main_quote_currency: Symbol,
    /// Whether to simulate order sending
    #[serde(default)]
    pub simulate_sender: bool,
    /// Whether to simulate chain operations
    #[serde(default)]
    pub simulate_chain: bool,
    /// Configuration files directory path
    #[serde(default = "default_config_path")]
    pub config_path: PathBuf,
}

/// Solver configuration data
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SolverConfigData {
    /// Price threshold for solver decisions
    #[serde(default = "default_price_threshold")]
    pub price_threshold: Decimal,
    /// Maximum levels for order book processing
    #[serde(default = "default_max_levels")]
    pub max_levels: usize,
    /// Fee factor for calculations
    #[serde(default = "default_fee_factor")]
    pub fee_factor: Decimal,
    /// Maximum order volley size
    #[serde(default = "default_max_order_volley_size")]
    pub max_order_volley_size: Decimal,
    /// Maximum volley size
    #[serde(default = "default_max_volley_size")]
    pub max_volley_size: Decimal,
    /// Minimum asset volley size
    #[serde(default = "default_min_asset_volley_size")]
    pub min_asset_volley_size: Decimal,
    /// Asset volley step size
    #[serde(default = "default_asset_volley_step_size")]
    pub asset_volley_step_size: Decimal,
    /// Maximum total volley size
    #[serde(default = "default_max_total_volley_size")]
    pub max_total_volley_size: Decimal,
    /// Minimum total volley available
    #[serde(default = "default_min_total_volley_available")]
    pub min_total_volley_available: Decimal,
    /// Zero threshold for calculations
    #[serde(default = "default_zero_threshold")]
    pub zero_threshold: Amount,
    /// Maximum batch size
    #[serde(default = "default_max_batch_size")]
    pub max_batch_size: usize,
    /// Fill threshold
    #[serde(default = "default_fill_threshold")]
    pub fill_threshold: Decimal,
    /// Mint threshold
    #[serde(default = "default_mint_threshold")]
    pub mint_threshold: Decimal,
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

/// Market data configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MarketDataConfigData {
    /// Market data provider settings
    pub provider: String,
    /// Connection timeout in seconds
    #[serde(default = "default_connection_timeout")]
    pub connection_timeout_secs: u64,
    /// Reconnection attempts
    #[serde(default = "default_reconnection_attempts")]
    pub reconnection_attempts: u32,
}

/// Chain connector configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChainConfigData {
    /// Chain provider (e.g., "ethereum", "arbitrum")
    pub provider: String,
    /// RPC URL for chain connection
    pub rpc_url: Option<String>,
    /// Private key for transactions (should be loaded from env)
    pub private_key: Option<String>,
    /// Gas price multiplier
    #[serde(default = "default_gas_price_multiplier")]
    pub gas_price_multiplier: f64,
}

/// Order sender configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrderSenderConfigData {
    /// Exchange provider
    pub provider: String,
    /// API credentials (should be loaded from env)
    pub api_key: Option<String>,
    pub api_secret: Option<String>,
    /// Rate limiting settings
    #[serde(default = "default_rate_limit_per_second")]
    pub rate_limit_per_second: u32,
}

/// Logging configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoggingConfig {
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

/// Server configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerConfigData {
    /// Bind address for the server
    #[serde(default = "default_bind_address")]
    pub bind_address: String,
    /// Server timeout settings
    #[serde(default = "default_server_timeout")]
    pub timeout_secs: u64,
}

// Default value functions for serde defaults
fn default_main_quote_currency() -> Symbol {
    "USDC".into()
}

fn default_config_path() -> PathBuf {
    "configs".into()
}

fn default_price_threshold() -> Decimal {
    rust_decimal::dec!(0.01)
}

fn default_max_levels() -> usize {
    5
}

fn default_fee_factor() -> Decimal {
    rust_decimal::dec!(1.002)
}

fn default_max_order_volley_size() -> Decimal {
    rust_decimal::dec!(20.0)
}

fn default_max_volley_size() -> Decimal {
    rust_decimal::dec!(100.0)
}

fn default_min_asset_volley_size() -> Decimal {
    rust_decimal::dec!(5.0)
}

fn default_asset_volley_step_size() -> Decimal {
    rust_decimal::dec!(0.1)
}

fn default_max_total_volley_size() -> Decimal {
    rust_decimal::dec!(1000.0)
}

fn default_min_total_volley_available() -> Decimal {
    rust_decimal::dec!(100.0)
}

fn default_zero_threshold() -> Amount {
    rust_decimal::dec!(0.0001)
}

fn default_max_batch_size() -> usize {
    10
}

fn default_fill_threshold() -> Decimal {
    rust_decimal::dec!(0.9999)
}

fn default_mint_threshold() -> Decimal {
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
    "info".to_string()
}

fn default_log_batch_size() -> usize {
    1000
}

fn default_bind_address() -> String {
    "127.0.0.1:3000".to_string()
}

fn default_server_timeout() -> u64 {
    30
}

impl Default for ApplicationConfig {
    fn default() -> Self {
        Self {
            app: AppConfig::default(),
            solver: SolverConfigData::default(),
            market_data: MarketDataConfigData::default(),
            chain: ChainConfigData::default(),
            order_sender: OrderSenderConfigData::default(),
            logging: LoggingConfig::default(),
            server: ServerConfigData::default(),
        }
    }
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            main_quote_currency: default_main_quote_currency(),
            simulate_sender: false,
            simulate_chain: false,
            config_path: default_config_path(),
        }
    }
}

impl Default for SolverConfigData {
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

impl Default for MarketDataConfigData {
    fn default() -> Self {
        Self {
            provider: "binance".to_string(),
            connection_timeout_secs: default_connection_timeout(),
            reconnection_attempts: default_reconnection_attempts(),
        }
    }
}

impl Default for ChainConfigData {
    fn default() -> Self {
        Self {
            provider: "ethereum".to_string(),
            rpc_url: None,
            private_key: None,
            gas_price_multiplier: default_gas_price_multiplier(),
        }
    }
}

impl Default for OrderSenderConfigData {
    fn default() -> Self {
        Self {
            provider: "binance".to_string(),
            api_key: None,
            api_secret: None,
            rate_limit_per_second: default_rate_limit_per_second(),
        }
    }
}

impl Default for LoggingConfig {
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

impl Default for ServerConfigData {
    fn default() -> Self {
        Self {
            bind_address: default_bind_address(),
            timeout_secs: default_server_timeout(),
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
        mut base_config: ApplicationConfig,
        file_path: &PathBuf,
    ) -> Result<ApplicationConfig, ConfigBuildError> {
        if !file_path.exists() {
            return Err(ConfigBuildError::FileError(format!(
                "Configuration file not found: {}",
                file_path.display()
            )));
        }

        let content = std::fs::read_to_string(file_path)?;

        let file_config: ApplicationConfig = match file_path.extension().and_then(|s| s.to_str()) {
            Some("json") => serde_json::from_str(&content)?,
            Some("toml") => toml::from_str(&content)?,
            Some(ext) => {
                return Err(ConfigBuildError::FileError(format!(
                    "Unsupported configuration file format: {}. Supported formats: json, toml",
                    ext
                )));
            }
            None => {
                return Err(ConfigBuildError::FileError(
                    "Configuration file has no extension. Supported formats: json, toml".to_string(),
                ));
            }
        };

        // Merge file config into base config
        // This is a simple merge - in a more sophisticated implementation,
        // you might want to do deep merging of nested structures
        self.merge_configs(base_config, file_config)
    }

    /// Merge environment variables into configuration
    fn merge_environment_variables(
        &self,
        mut config: ApplicationConfig,
    ) -> Result<ApplicationConfig, ConfigBuildError> {
        use std::env;

        // App-level environment variables
        if let Ok(val) = env::var("INDEX_MAKER_MAIN_QUOTE_CURRENCY") {
            config.app.main_quote_currency = val.into();
        }
        if let Ok(val) = env::var("INDEX_MAKER_SIMULATE_SENDER") {
            config.app.simulate_sender = val.parse().unwrap_or(false);
        }
        if let Ok(val) = env::var("INDEX_MAKER_SIMULATE_CHAIN") {
            config.app.simulate_chain = val.parse().unwrap_or(false);
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

    /// Merge two configurations, with the second taking precedence
    fn merge_configs(
        &self,
        mut base: ApplicationConfig,
        override_config: ApplicationConfig,
    ) -> Result<ApplicationConfig, ConfigBuildError> {
        // For now, we do a simple field-by-field merge
        // In a more sophisticated implementation, you might want to use a merge library
        // or implement custom merge logic for nested structures

        // Note: This is a simplified merge. For production use, consider using
        // a library like `merge` or implementing more sophisticated merging logic
        Ok(override_config) // For now, just use the override config entirely
    }

    /// Validate the final configuration
    fn validate_config(&self, config: &ApplicationConfig) -> Result<(), ConfigBuildError> {
        // Validate solver configuration
        if config.solver.price_threshold <= rust_decimal::Decimal::ZERO {
            return Err(ConfigBuildError::ValidationError(
                "price_threshold must be positive".to_string(),
            ));
        }

        if config.solver.max_levels == 0 {
            return Err(ConfigBuildError::ValidationError(
                "max_levels must be greater than 0".to_string(),
            ));
        }

        if config.solver.fee_factor <= rust_decimal::Decimal::ONE {
            return Err(ConfigBuildError::ValidationError(
                "fee_factor must be greater than 1.0".to_string(),
            ));
        }

        if config.solver.max_batch_size == 0 {
            return Err(ConfigBuildError::ValidationError(
                "max_batch_size must be greater than 0".to_string(),
            ));
        }

        // Validate market data configuration
        if config.market_data.provider.is_empty() {
            return Err(ConfigBuildError::ValidationError(
                "market_data.provider cannot be empty".to_string(),
            ));
        }

        // Validate chain configuration
        if config.chain.provider.is_empty() {
            return Err(ConfigBuildError::ValidationError(
                "chain.provider cannot be empty".to_string(),
            ));
        }

        // Validate order sender configuration
        if config.order_sender.provider.is_empty() {
            return Err(ConfigBuildError::ValidationError(
                "order_sender.provider cannot be empty".to_string(),
            ));
        }

        // Validate logging configuration
        let valid_log_levels = ["trace", "debug", "info", "warn", "error"];
        if !valid_log_levels.contains(&config.logging.level.as_str()) {
            return Err(ConfigBuildError::ValidationError(format!(
                "Invalid log level '{}'. Valid levels: {}",
                config.logging.level,
                valid_log_levels.join(", ")
            )));
        }

        // Validate server configuration
        if config.server.bind_address.is_empty() {
            return Err(ConfigBuildError::ValidationError(
                "server.bind_address cannot be empty".to_string(),
            ));
        }

        // Validate bind address format (basic check)
        if !config.server.bind_address.contains(':') {
            return Err(ConfigBuildError::ValidationError(
                "server.bind_address must be in format 'host:port'".to_string(),
            ));
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
#[derive(Debug, Clone)]
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
    pub fn from_cli(cli: &crate::Cli) -> Self {
        Self {
            main_quote_currency: cli.main_quote_currency.clone(),
            simulate_sender: Some(cli.simulate_sender),
            simulate_chain: Some(cli.simulate_chain),
            bind_address: cli.bind_address.clone(),
            log_path: cli.log_path.as_ref().map(PathBuf::from),
            term_log_off: Some(cli.term_log_off),
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
