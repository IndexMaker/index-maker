use alloy::primitives::{address, Address};
use dotenv::{dotenv, from_path};
use serde::{Deserialize, Serialize};
use std::env;
use std::str::FromStr;

/// Configuration for the EVM Connector
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvmConnectorConfig {
    /// Maximum number of concurrent chain operations
    pub max_operations: usize,
    /// Chain configurations for supported networks
    pub chains: Vec<ChainConfig>,
    /// Default bridge configuration
    pub bridge: BridgeConfig,
}

/// Configuration for a specific blockchain
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChainConfig {
    /// Chain ID (e.g., 42161 for Arbitrum, 8453 for Base)
    pub chain_id: u32,
    /// Human-readable chain name
    pub name: String,
    /// RPC URL for this chain
    pub rpc_url: String,
    /// Native token symbol (e.g., "ETH")
    pub native_symbol: String,
    /// Gas price multiplier for fee estimation
    pub gas_price_multiplier: f64,
}

/// Bridge configuration containing settings for different bridge types
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BridgeConfig {
    /// Across protocol bridge configuration
    pub across: AcrossBridgeConfig,
    /// ERC20 bridge configuration
    pub erc20: Erc20BridgeConfig,
}

/// Configuration for Across protocol bridges
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AcrossBridgeConfig {
    /// Across connector contract address
    pub connector_address: Address,
    /// Across spoke pool contract address  
    pub spoke_pool_address: Address,
    /// OTC custody contract address
    pub custody_address: Address,
    pub usdc_arbitrum_address: Address,
    pub usdc_base_address: Address,
    /// API URL for fee suggestions
    pub api_url: String,
    /// Default timeout for transactions (seconds)
    pub transaction_timeout: u64,
}

/// Configuration for ERC20 bridges (same-chain transfers)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Erc20BridgeConfig {
    /// Default gas limit for ERC20 transfers
    pub default_gas_limit: u64,
    /// Token contract addresses by chain ID and symbol
    pub token_addresses:
        std::collections::HashMap<String, std::collections::HashMap<String, Address>>,
    /// Default slippage tolerance (basis points)
    pub slippage_tolerance: u16,
}

impl Default for EvmConnectorConfig {
    fn default() -> Self {
        Self::from_env().unwrap_or_else(|e| {
            tracing::warn!(
                "Failed to load EvmConnectorConfig from environment: {}, using fallback",
                e
            );
            Self::fallback_default()
        })
    }
}

impl EvmConnectorConfig {
    /// Load configuration from environment variables
    pub fn from_env() -> Result<Self, Box<dyn std::error::Error>> {
        // Load environment variables from .env file
        Self::load_env_file();

        let max_operations = env::var("MAX_OPERATIONS")
            .map_err(|_| "MAX_OPERATIONS environment variable not set")?
            .parse::<usize>()?;

        let chains = vec![
            ChainConfig {
                chain_id: env::var("ARBITRUM_CHAIN_ID")
                    .map_err(|_| "ARBITRUM_CHAIN_ID environment variable not set")?
                    .parse::<u32>()?,
                name: "Arbitrum".to_string(),
                rpc_url: env::var("ARBITRUM_RPC_URL")
                    .map_err(|_| "ARBITRUM_RPC_URL environment variable not set")?,
                native_symbol: "ETH".to_string(),
                gas_price_multiplier: 1.1,
            },
            ChainConfig {
                chain_id: env::var("BASE_CHAIN_ID")
                    .map_err(|_| "BASE_CHAIN_ID environment variable not set")?
                    .parse::<u32>()?,
                name: "Base".to_string(),
                rpc_url: env::var("BASE_RPC_URL")
                    .map_err(|_| "BASE_RPC_URL environment variable not set")?,
                native_symbol: "ETH".to_string(),
                gas_price_multiplier: 1.1,
            },
        ];

        Ok(Self {
            max_operations,
            chains,
            bridge: BridgeConfig::from_env()?,
        })
    }

    /// Fallback default configuration (used when env vars are not available)
    fn fallback_default() -> Self {
        Self {
            max_operations: 10,
            chains: vec![
                ChainConfig {
                    chain_id: 42161,
                    name: "Arbitrum".to_string(),
                    rpc_url: "https://arb1.lava.build".to_string(),
                    native_symbol: "ETH".to_string(),
                    gas_price_multiplier: 1.1,
                },
                ChainConfig {
                    chain_id: 8453,
                    name: "Base".to_string(),
                    rpc_url: "https://base.llamarpc.com".to_string(),
                    native_symbol: "ETH".to_string(),
                    gas_price_multiplier: 1.1,
                },
            ],
            bridge: BridgeConfig::fallback_default(),
        }
    }
}

impl Default for BridgeConfig {
    fn default() -> Self {
        Self::from_env().unwrap_or_else(|_| Self::fallback_default())
    }
}

impl BridgeConfig {
    /// Load bridge configuration from environment variables
    pub fn from_env() -> Result<Self, Box<dyn std::error::Error>> {
        Ok(Self {
            across: AcrossBridgeConfig::from_env()?,
            erc20: Erc20BridgeConfig::from_env()?,
        })
    }

    /// Fallback default configuration
    fn fallback_default() -> Self {
        Self {
            across: AcrossBridgeConfig::fallback_default(),
            erc20: Erc20BridgeConfig::fallback_default(),
        }
    }
}

impl Default for AcrossBridgeConfig {
    fn default() -> Self {
        Self::from_env().unwrap_or_else(|_| Self::fallback_default())
    }
}

impl AcrossBridgeConfig {
    /// Load Across bridge configuration from environment variables
    pub fn from_env() -> Result<Self, Box<dyn std::error::Error>> {
        Ok(Self {
            connector_address: Address::from_str(
                &env::var("ACROSS_CONNECTOR_ADDRESS")
                    .map_err(|_| "ACROSS_CONNECTOR_ADDRESS environment variable not set")?,
            )?,
            spoke_pool_address: Address::from_str(
                &env::var("ACROSS_SPOKE_POOL_ADDRESS")
                    .map_err(|_| "ACROSS_SPOKE_POOL_ADDRESS environment variable not set")?,
            )?,
            custody_address: Address::from_str(
                &env::var("OTC_CUSTODY_ADDRESS")
                    .map_err(|_| "OTC_CUSTODY_ADDRESS environment variable not set")?,
            )?,
            usdc_arbitrum_address: Address::from_str(
                &env::var("USDC_ARBITRUM_ADDRESS")
                    .map_err(|_| "USDC_ARBITRUM_ADDRESS environment variable not set")?,
            )?,
            usdc_base_address: Address::from_str(
                &env::var("USDC_BASE_ADDRESS")
                    .map_err(|_| "USDC_BASE_ADDRESS environment variable not set")?,
            )?,
            api_url: env::var("ACROSS_API_URL")
                .map_err(|_| "ACROSS_API_URL environment variable not set")?,
            transaction_timeout: env::var("DEFAULT_TRANSACTION_TIMEOUT")
                .map_err(|_| "DEFAULT_TRANSACTION_TIMEOUT environment variable not set")?
                .parse::<u64>()?,
        })
    }

    /// Fallback default configuration
    fn fallback_default() -> Self {
        Self {
            connector_address: address!("0x8350a9Ab669808BE1DDF24FAF9c14475321D0504"),
            spoke_pool_address: address!("0xe35e9842fceaCA96570B734083f4a58e8F7C5f2A"),
            custody_address: address!("0x9F6754bB627c726B4d2157e90357282d03362BCd"),
            usdc_arbitrum_address: address!("0xaf88d065e77c8cC2239327C5EDb3A432268e5831"),
            usdc_base_address: address!("0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913"),
            api_url: "https://app.across.to/api/suggested-fees".to_string(),
            transaction_timeout: 3600,
        }
    }
}

impl Default for Erc20BridgeConfig {
    fn default() -> Self {
        Self::from_env().unwrap_or_else(|_| Self::fallback_default())
    }
}

impl Erc20BridgeConfig {
    /// Get token address by chain name and token symbol (case-insensitive)
    pub fn get_token_address_by_name(
        &self,
        chain_name: &str,
        token_symbol: &str,
    ) -> Option<Address> {
        self.token_addresses
            .get(chain_name)?
            .get(&token_symbol.to_uppercase())
            .copied()
    }

    /// Load ERC20 bridge configuration from environment variables
    pub fn from_env() -> Result<Self, Box<dyn std::error::Error>> {
        let mut token_addresses = std::collections::HashMap::new();

        // Arbitrum tokens
        let mut arbitrum_tokens = std::collections::HashMap::new();
        arbitrum_tokens.insert(
            "USDC".to_string(),
            Address::from_str(
                &env::var("USDC_ARBITRUM_ADDRESS")
                    .map_err(|_| "USDC_ARBITRUM_ADDRESS environment variable not set")?,
            )?,
        );
        token_addresses.insert("arbitrum".to_string(), arbitrum_tokens);

        // Base tokens
        let mut base_tokens = std::collections::HashMap::new();
        base_tokens.insert(
            "USDC".to_string(),
            Address::from_str(
                &env::var("USDC_BASE_ADDRESS")
                    .map_err(|_| "USDC_BASE_ADDRESS environment variable not set")?,
            )?,
        );
        token_addresses.insert("base".to_string(), base_tokens);

        Ok(Self {
            default_gas_limit: 100_000,
            token_addresses,
            slippage_tolerance: 50, // 0.5%
        })
    }

    /// Fallback default configuration
    fn fallback_default() -> Self {
        let mut token_addresses = std::collections::HashMap::new();

        // Arbitrum tokens
        let mut arbitrum_tokens = std::collections::HashMap::new();
        arbitrum_tokens.insert(
            "USDC".to_string(),
            "0xaf88d065e77c8cC2239327C5EDb3A432268e5831"
                .parse()
                .unwrap(),
        );
        token_addresses.insert("arbitrum".to_string(), arbitrum_tokens);

        // Base tokens
        let mut base_tokens = std::collections::HashMap::new();
        base_tokens.insert(
            "USDC".to_string(),
            "0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913"
                .parse()
                .unwrap(),
        );
        token_addresses.insert("base".to_string(), base_tokens);

        Self {
            default_gas_limit: 100_000,
            token_addresses,
            slippage_tolerance: 50, // 0.5%
        }
    }
}

impl EvmConnectorConfig {
    /// Get chain configuration by chain name (case-insensitive)
    pub fn get_chain_config(&self, chain_name: &str) -> Option<&ChainConfig> {
        self.chains
            .iter()
            .find(|chain| chain.name.to_lowercase() == chain_name.to_lowercase())
    }

    /// Get chain ID by chain name (case-insensitive)
    pub fn get_chain_id(&self, chain_name: &str) -> Option<u32> {
        self.get_chain_config(chain_name)
            .map(|config| config.chain_id)
    }

    /// Get RPC URL by chain name (case-insensitive)
    pub fn get_rpc_url(&self, chain_name: &str) -> Option<String> {
        self.get_chain_config(chain_name)
            .map(|config| config.rpc_url.clone())
    }

    /// Get USDC address for a specific chain by chain name (case-insensitive)
    pub fn get_usdc_address(&self, chain_name: &str) -> Option<Address> {
        self.get_chain_config(chain_name).map(|config| {
            if config.name.to_lowercase() == "arbitrum" {
                self.bridge.across.usdc_arbitrum_address
            } else {
                self.bridge.across.usdc_base_address
            }
        })
    }

    /// Get ERC20 token address by chain name and token symbol (case-insensitive)
    pub fn get_token_address(&self, chain_name: &str, token_symbol: &str) -> Option<Address> {
        self.bridge
            .erc20
            .get_token_address_by_name(chain_name, token_symbol)
    }

    /// Get all supported chain names
    pub fn get_supported_chain_names(&self) -> Vec<String> {
        self.chains.iter().map(|chain| chain.name.clone()).collect()
    }

    /// Check if a chain name is supported (case-insensitive)
    pub fn is_chain_supported(&self, chain_name: &str) -> bool {
        self.get_chain_config(chain_name).is_some()
    }

    /// Load .env file from the correct location
    fn load_env_file() {
        // Try to load .env from the alloy-evm-connector directory first
        if let Ok(current_dir) = env::current_dir() {
            let alloy_env_path = current_dir.join("libs/alloy-evm-connector/.env");
            if alloy_env_path.exists() {
                from_path(alloy_env_path).ok();
            } else {
                // Fallback to current directory
                dotenv().ok();
            }
        } else {
            dotenv().ok();
        }
    }

    /// Get default RPC URL from environment
    pub fn get_default_rpc_url() -> String {
        // Ensure .env file is loaded from the correct location
        Self::load_env_file();
        env::var("DEFAULT_RPC_URL").expect("DEFAULT_RPC_URL environment variable not set")
    }

    /// Get default sender address from environment (for testing)
    pub fn get_default_sender_address() -> Address {
        let addr_str = env::var("DEFAULT_SENDER_ADDRESS")
            .expect("DEFAULT_SENDER_ADDRESS environment variable not set");
        Address::from_str(&addr_str).expect("Invalid DEFAULT_SENDER_ADDRESS format")
    }

    /// Get ETH to USDC conversion rate from environment
    pub fn get_eth_to_usdc_rate() -> u32 {
        env::var("ETH_TO_USDC_RATE")
            .expect("ETH_TO_USDC_RATE environment variable not set")
            .parse()
            .expect("Invalid ETH_TO_USDC_RATE format")
    }

    /// Get fill deadline buffer from environment
    pub fn get_filldeadline_buffer() -> u64 {
        env::var("DEFAULT_FILLDEADLINE_BUFFER")
            .expect("DEFAULT_FILLDEADLINE_BUFFER environment variable not set")
            .parse()
            .expect("Invalid DEFAULT_FILLDEADLINE_BUFFER format")
    }

    /// Get exclusivity deadline buffer from environment
    pub fn get_exclusivity_deadline_buffer() -> u64 {
        env::var("DEFAULT_EXCLUSIVITY_DEADLINE_BUFFER")
            .expect("DEFAULT_EXCLUSIVITY_DEADLINE_BUFFER environment variable not set")
            .parse()
            .expect("Invalid DEFAULT_EXCLUSIVITY_DEADLINE_BUFFER format")
    }

    /// Get private key from environment (for credentials)
    /// This is the only place in the system that should access private key env vars
    pub fn get_private_key() -> String {
        // Ensure .env file is loaded from the correct location
        Self::load_env_file();
        env::var("PRIVATE_KEY").expect("PRIVATE_KEY environment variable not set")
    }

    /// Get chain-specific private key from environment with fallback to generic
    pub fn get_chain_private_key(prefix: &str) -> String {
        // Ensure .env file is loaded
        Self::load_env_file();

        let key_var = format!("{}_PRIVATE_KEY", prefix);

        // Try chain-specific key first, then fall back to generic
        env::var(&key_var)
            .or_else(|_| env::var("PRIVATE_KEY"))
            .unwrap_or_else(|_| {
                panic!(
                    "Either {} or PRIVATE_KEY environment variable must be set",
                    key_var
                )
            })
    }

    /// Check if private key is available (for validation)
    pub fn has_private_key() -> bool {
        env::var("PRIVATE_KEY").is_ok()
    }

    /// Check if chain-specific private key is available
    pub fn has_chain_private_key(prefix: &str) -> bool {
        let key_var = format!("{}_PRIVATE_KEY", prefix);
        env::var(&key_var).is_ok() || Self::has_private_key()
    }
}
