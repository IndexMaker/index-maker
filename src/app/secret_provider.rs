use eyre::Result;

/// Trait for providing secrets without storing them
///
/// This trait ensures secrets are never cached in memory and are always
/// retrieved fresh from their source (environment variables, key vaults, etc.)
pub trait SecretProvider: Send + Sync {
    /// Get Binance API key from secure source
    fn get_binance_api_key(&self) -> Result<String>;

    /// Get Binance API secret from secure source
    fn get_binance_api_secret(&self) -> Result<String>;

    /// Get Bitget API key from secure source
    fn get_bitget_api_key(&self) -> Result<String>;

    /// Get Bitget API secret from secure source
    fn get_bitget_api_secret(&self) -> Result<String>;

    /// Get blockchain private key from secure source
    fn get_chain_private_key(&self) -> Result<String>;

    /// Check if Binance credentials are available
    fn has_binance_credentials(&self) -> bool {
        self.get_binance_api_key().is_ok() && self.get_binance_api_secret().is_ok()
    }

    /// Check if Bitget credentials are available
    fn has_bitget_credentials(&self) -> bool {
        self.get_bitget_api_key().is_ok() && self.get_bitget_api_secret().is_ok()
    }

    /// Check if chain credentials are available
    fn has_chain_credentials(&self) -> bool {
        self.get_chain_private_key().is_ok()
    }
}

/// Environment variable-based secret provider
///
/// This implementation reads secrets from environment variables
/// without caching them in memory, following security best practices
pub struct EnvSecretProvider;

impl EnvSecretProvider {
    /// Create a new environment-based secret provider
    pub fn new() -> Self {
        Self
    }
}

impl Default for EnvSecretProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl SecretProvider for EnvSecretProvider {
    fn get_binance_api_key(&self) -> Result<String> {
        std::env::var("BINANCE_API_KEY")
            .map_err(|_| eyre::eyre!("Failed to get BINANCE_API_KEY secret"))
    }

    fn get_binance_api_secret(&self) -> Result<String> {
        std::env::var("BINANCE_API_SECRET")
            .map_err(|_| eyre::eyre!("Failed to get BINANCE_API_SECRET secret"))
    }

    fn get_bitget_api_key(&self) -> Result<String> {
        std::env::var("BITGET_API_KEY")
            .map_err(|_| eyre::eyre!("Failed to get BITGET_API_KEY secret"))
    }

    fn get_bitget_api_secret(&self) -> Result<String> {
        std::env::var("BITGET_API_SECRET")
            .map_err(|_| eyre::eyre!("Failed to get BITGET_API_SECRET secret"))
    }

    fn get_chain_private_key(&self) -> Result<String> {
        std::env::var("CHAIN_PRIVATE_KEY")
            .map_err(|_| eyre::eyre!("Failed to get CHAIN_PRIVATE_KEY secret"))
    }
}

/// Mock secret provider for testing
///
/// This implementation provides fake secrets for testing purposes
/// Still follows the no-caching principle by generating secrets on demand
#[cfg(test)]
pub struct MockSecretProvider {
    pub binance_available: bool,
    pub bitget_available: bool,
    pub chain_available: bool,
}

#[cfg(test)]
impl MockSecretProvider {
    pub fn new() -> Self {
        Self {
            binance_available: true,
            bitget_available: true,
            chain_available: true,
        }
    }

    pub fn with_binance(mut self, available: bool) -> Self {
        self.binance_available = available;
        self
    }

    pub fn with_bitget(mut self, available: bool) -> Self {
        self.bitget_available = available;
        self
    }

    pub fn with_chain(mut self, available: bool) -> Self {
        self.chain_available = available;
        self
    }
}

#[cfg(test)]
impl SecretProvider for MockSecretProvider {
    fn get_binance_api_key(&self) -> Result<String> {
        if self.binance_available {
            Ok(String::from("mock_binance_key"))
        } else {
            Err(eyre::eyre!("Mock Binance API key not available"))
        }
    }

    fn get_binance_api_secret(&self) -> Result<String> {
        if self.binance_available {
            Ok(String::from("mock_binance_secret"))
        } else {
            Err(eyre::eyre!("Mock Binance API secret not available"))
        }
    }

    fn get_bitget_api_key(&self) -> Result<String> {
        if self.bitget_available {
            Ok(String::from("mock_bitget_key"))
        } else {
            Err(eyre::eyre!("Mock Bitget API key not available"))
        }
    }

    fn get_bitget_api_secret(&self) -> Result<String> {
        if self.bitget_available {
            Ok(String::from("mock_bitget_secret"))
        } else {
            Err(eyre::eyre!("Mock Bitget API secret not available"))
        }
    }

    fn get_chain_private_key(&self) -> Result<String> {
        if self.chain_available {
            Ok(String::from("0x1234567890abcdef1234567890abcdef12345678"))
        } else {
            Err(eyre::eyre!("Mock chain private key not available"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mock_secret_provider() {
        let provider = MockSecretProvider::new();

        assert!(provider.has_binance_credentials());
        assert!(provider.has_bitget_credentials());
        assert!(provider.has_chain_credentials());

        assert!(provider.get_binance_api_key().is_ok());
        assert!(provider.get_binance_api_secret().is_ok());
        assert!(provider.get_bitget_api_key().is_ok());
        assert!(provider.get_bitget_api_secret().is_ok());
        assert!(provider.get_chain_private_key().is_ok());
    }

    #[test]
    fn test_mock_secret_provider_unavailable() {
        let provider = MockSecretProvider::new()
            .with_binance(false)
            .with_bitget(false)
            .with_chain(false);

        assert!(!provider.has_binance_credentials());
        assert!(!provider.has_bitget_credentials());
        assert!(!provider.has_chain_credentials());

        assert!(provider.get_binance_api_key().is_err());
        assert!(provider.get_binance_api_secret().is_err());
        assert!(provider.get_bitget_api_key().is_err());
        assert!(provider.get_bitget_api_secret().is_err());
        assert!(provider.get_chain_private_key().is_err());
    }
}
