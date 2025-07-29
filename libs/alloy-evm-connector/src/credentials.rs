use std::env;
use dotenv::dotenv;
use eyre::Result;
use crate::config::EvmConnectorConfig;

/// EVM blockchain credentials for connecting to chains
/// Follows the same pattern as binance credentials with lazy loading for security
#[derive(Clone)]
pub struct EvmCredentials {
    /// Chain ID for this credential set
    pub chain_id: u64,
    /// RPC endpoint URL
    pub rpc_url: String,
    /// Lazy-loaded private key function for security
    get_private_key_fn: std::sync::Arc<dyn Fn() -> String + Send + Sync>,
}

impl EvmCredentials {
    /// Create new EVM credentials with lazy private key loading
    /// Following the exact binance pattern for security
    pub fn new(
        chain_id: u64,
        rpc_url: String,
        get_private_key_fn: impl Fn() -> String + Send + Sync + 'static,
    ) -> Self {
        Self {
            chain_id,
            rpc_url,
            get_private_key_fn: std::sync::Arc::new(get_private_key_fn),
        }
    }

    /// Get the RPC URL for this chain
    pub fn get_rpc_url(&self) -> String {
        self.rpc_url.clone()
    }

    /// Get the chain ID
    pub fn get_chain_id(&self) -> u64 {
        self.chain_id
    }

    /// Get the private key (loaded from environment at runtime for security)
    /// Following the binance pattern of lazy loading secrets
    pub(crate) fn get_private_key(&self) -> String {
        (*self.get_private_key_fn)()
    }

    /// Create credentials from environment variables
    /// Follows the binance pattern for environment variable loading
    pub fn from_env(chain_id: u64) -> Result<Self> {
        // Load environment variables from .env file (try multiple locations)
        dotenv::from_path(".env").ok()
            .or_else(|| dotenv::from_path("../.env").ok())
            .or_else(|| dotenv::from_path("../../.env").ok());
        
        // Get RPC URL with fallback (following the current pattern)
        let rpc_url = env::var("RPC_URL")
            .unwrap_or_else(|_| EvmConnectorConfig::get_default_rpc_url());
        
        // Validate that private key environment variable exists
        // This follows the binance pattern of early validation
        env::var("PRIVATE_KEY")
            .map_err(|_| eyre::eyre!("PRIVATE_KEY environment variable not set"))?;
        
        Ok(Self::new(chain_id, rpc_url, move || {
            env::var("PRIVATE_KEY").expect("PRIVATE_KEY environment variable not set")
        }))
    }

    /// Create credentials from environment variables with custom RPC URL
    /// Allows for chain-specific RPC endpoints
    pub fn from_env_with_rpc(chain_id: u64, rpc_url: String) -> Result<Self> {
        // Load environment variables from .env file (try multiple locations)
        dotenv::from_path(".env").ok()
            .or_else(|| dotenv::from_path("../.env").ok())
            .or_else(|| dotenv::from_path("../../.env").ok());
        
        // Validate that private key environment variable exists
        env::var("PRIVATE_KEY")
            .map_err(|_| eyre::eyre!("PRIVATE_KEY environment variable not set"))?;
        
        Ok(Self::new(chain_id, rpc_url, move || {
            env::var("PRIVATE_KEY").expect("PRIVATE_KEY environment variable not set")
        }))
    }

    /// Create credentials for specific chains with environment variable patterns
    /// Following the pattern used in binance examples
    pub fn ethereum_mainnet() -> Result<Self> {
        Self::from_env_with_chain_prefix(1, "ETHEREUM")
    }

    pub fn arbitrum() -> Result<Self> {
        let config = EvmConnectorConfig::default();
        let chain_id = config.get_chain_config(42161).map(|c| c.chain_id as u64).unwrap_or(42161);
        Self::from_env_with_chain_prefix(chain_id, "ARBITRUM")
    }

    pub fn base() -> Result<Self> {
        let config = EvmConnectorConfig::default();
        let chain_id = config.get_chain_config(8453).map(|c| c.chain_id as u64).unwrap_or(8453);
        Self::from_env_with_chain_prefix(chain_id, "BASE")
    }

    pub fn polygon() -> Result<Self> {
        Self::from_env_with_chain_prefix(137, "POLYGON")
    }

    /// Create credentials with chain-specific environment variable prefix
    /// Allows for multiple chain configurations like:
    /// ETHEREUM_RPC_URL, ETHEREUM_PRIVATE_KEY
    /// ARBITRUM_RPC_URL, ARBITRUM_PRIVATE_KEY
    fn from_env_with_chain_prefix(chain_id: u64, prefix: &str) -> Result<Self> {
        dotenv::from_path(".env").ok()
            .or_else(|| dotenv::from_path("../.env").ok())
            .or_else(|| dotenv::from_path("../../.env").ok());
        
        let rpc_var = format!("{}_RPC_URL", prefix);
        let key_var = format!("{}_PRIVATE_KEY", prefix);
        
        // Get RPC URL with fallback to generic RPC_URL
        let rpc_url = env::var(&rpc_var)
            .or_else(|_| env::var("RPC_URL"))
            .unwrap_or_else(|_| EvmConnectorConfig::get_default_rpc_url());
        
        // Validate private key exists (chain-specific or generic)
        let has_chain_key = env::var(&key_var).is_ok();
        let has_generic_key = env::var("PRIVATE_KEY").is_ok();
        
        if !has_chain_key && !has_generic_key {
            return Err(eyre::eyre!(
                "Neither {} nor PRIVATE_KEY environment variable is set", 
                key_var
            ));
        }
        
        Ok(Self::new(chain_id, rpc_url, move || {
            // Try chain-specific key first, then fall back to generic
            env::var(&key_var)
                .or_else(|_| env::var("PRIVATE_KEY"))
                .expect(&format!("Either {} or PRIVATE_KEY environment variable must be set", key_var))
        }))
    }
}

impl std::fmt::Debug for EvmCredentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EvmCredentials")
            .field("chain_id", &self.chain_id)
            .field("rpc_url", &self.rpc_url)
            .field("get_private_key_fn", &"<hidden>")
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;

    #[test]
    fn test_credentials_creation() {
        let rpc_url = EvmConnectorConfig::get_default_rpc_url();
        let credentials = EvmCredentials::new(1, rpc_url.clone(), || {
            "test_private_key".to_string()
        });

        assert_eq!(credentials.get_chain_id(), 1);
        assert_eq!(credentials.get_rpc_url(), rpc_url);
        assert_eq!(credentials.get_private_key(), "test_private_key");
    }

    #[test]
    fn test_from_env_validation() {
        // Clear environment variables
        env::remove_var("PRIVATE_KEY");
        env::remove_var("RPC_URL");

        // Should fail when PRIVATE_KEY is not set
        let result = EvmCredentials::from_env(1);
        assert!(result.is_err());
    }

    #[test]
    fn test_chain_specific_credentials() {
        // Test that the predefined chain methods exist
        env::set_var("PRIVATE_KEY", "test_key");
        
        let ethereum = EvmCredentials::ethereum_mainnet();
        assert!(ethereum.is_ok());
        assert_eq!(ethereum.unwrap().get_chain_id(), 1);

        let arbitrum = EvmCredentials::arbitrum();
        assert!(arbitrum.is_ok());
        let expected_chain_id = EvmConnectorConfig::default().get_chain_config(42161).map(|c| c.chain_id as u64).unwrap_or(42161);
        assert_eq!(arbitrum.unwrap().get_chain_id(), expected_chain_id);

        // Clean up
        env::remove_var("PRIVATE_KEY");
    }
}