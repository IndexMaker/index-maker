use crate::config::EvmConnectorConfig;
use eyre::Result;

/// EVM blockchain credentials for connecting to chains
/// Follows the same pattern as binance credentials with lazy loading for security
#[derive(Clone)]
pub struct EvmCredentials {
    /// Chain ID for this credential set
    pub chain_id: u32,
    /// RPC endpoint URL
    pub rpc_url: String,
    /// Lazy-loaded private key function for security
    get_private_key_fn: std::sync::Arc<dyn Fn() -> String + Send + Sync>,
}

impl EvmCredentials {
    /// Create new EVM credentials with lazy private key loading
    /// Following the exact binance pattern for security
    pub fn new(
        chain_id: u32,
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
    pub fn get_chain_id(&self) -> u32 {
        self.chain_id
    }

    /// Get the private key (loaded from environment at runtime for security)
    /// Following the binance pattern of lazy loading secrets
    pub(crate) fn get_private_key(&self) -> String {
        (*self.get_private_key_fn)()
    }

    /// Create credentials from config system
    /// Uses EvmConnectorConfig as the single source of truth for all configuration
    pub fn from_env(chain_name: &str) -> Result<Self> {
        // Try to use config system first (recommended approach)
        Self::from_config(chain_name).or_else(|_| {
            // Fallback to generic configuration for unknown chains
            let rpc_url = EvmConnectorConfig::get_default_rpc_url();

            // Validate that private key is available
            if !EvmConnectorConfig::has_private_key() {
                return Err(eyre::eyre!("PRIVATE_KEY environment variable not set"));
            }

            let chain_id = EvmConnectorConfig::default()
                .get_chain_id(chain_name)
                .unwrap();

            Ok(Self::new(chain_id, rpc_url, move || {
                EvmConnectorConfig::get_private_key()
            }))
        })
    }

    /// Create credentials with custom RPC URL
    /// Uses config system for private key management
    pub fn from_env_with_rpc(chain_id: u32, rpc_url: String) -> Result<Self> {
        // Validate that private key is available through config system
        if !EvmConnectorConfig::has_private_key() {
            return Err(eyre::eyre!("PRIVATE_KEY environment variable not set"));
        }

        Ok(Self::new(chain_id, rpc_url, move || {
            EvmConnectorConfig::get_private_key()
        }))
    }

    /// Create credentials from config system (recommended approach)
    /// Uses EvmConnectorConfig as the single source of truth for chain configurations
    pub fn from_config(chain_name: &str) -> Result<Self> {
        let config = EvmConnectorConfig::default();
        let chain_config = config
            .get_chain_config(chain_name)
            .ok_or_else(|| eyre::eyre!("Unsupported chain ID: {}", chain_name))?;

        Self::from_env_with_rpc(chain_config.chain_id, chain_config.rpc_url.clone())
    }

    pub fn arbitrum() -> Result<Self> {
        Self::from_config("arbitrum")
            .or_else(|_| Self::from_env_with_chain_prefix("arbitrum", "ARBITRUM"))
    }

    pub fn base() -> Result<Self> {
        Self::from_config("base").or_else(|_| Self::from_env_with_chain_prefix("base", "BASE"))
    }

    /// Create credentials with chain-specific configuration
    /// Uses config system for both RPC URL and private key resolution
    fn from_env_with_chain_prefix(chain_name: &str, prefix: &str) -> Result<Self> {
        // Get RPC URL from config system with fallback to default
        let rpc_url = EvmConnectorConfig::default()
            .get_chain_config(chain_name)
            .map(|config| config.rpc_url.clone())
            .unwrap_or_else(|| EvmConnectorConfig::get_default_rpc_url());

        // Validate private key exists (chain-specific or generic) through config system
        if !EvmConnectorConfig::has_chain_private_key(prefix) {
            return Err(eyre::eyre!(
                "Neither {}_PRIVATE_KEY nor PRIVATE_KEY environment variable is set",
                prefix
            ));
        }

        let chain_id = EvmConnectorConfig::default()
            .get_chain_id(chain_name)
            .unwrap();

        let prefix_owned = prefix.to_string();
        Ok(Self::new(chain_id, rpc_url, move || {
            EvmConnectorConfig::get_chain_private_key(&prefix_owned)
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
