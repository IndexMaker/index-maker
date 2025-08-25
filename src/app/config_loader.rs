use std::path::PathBuf;
use eyre::Result;

use super::config::{ApplicationConfig, ConfigBuildError, ConfigLoader, CliOverrides};

/// High-level configuration loading interface that provides convenient methods
/// for loading configuration from various sources
pub struct ApplicationConfigLoader {
    loader: ConfigLoader,
}

impl ApplicationConfigLoader {
    /// Create a new configuration loader
    pub fn new() -> Self {
        Self {
            loader: ConfigLoader::new(),
        }
    }

    /// Load configuration from a file path
    pub fn from_file<P: Into<PathBuf>>(path: P) -> Result<ApplicationConfig, ConfigBuildError> {
        let loader = Self::new();
        let path = path.into();
        loader.loader.load_config(Some(&path), None)
    }

    /// Load configuration from file with CLI overrides
    pub fn from_file_with_cli<P: Into<PathBuf>>(
        path: P,
        cli: &crate::Cli,
    ) -> Result<ApplicationConfig, ConfigBuildError> {
        let loader = Self::new();
        let path = path.into();
        let cli_overrides = CliOverrides::from_cli(cli);
        loader.loader.load_config(Some(&path), Some(&cli_overrides))
    }

    /// Load configuration from CLI arguments only (with defaults)
    pub fn from_cli(cli: &crate::Cli) -> Result<ApplicationConfig, ConfigBuildError> {
        let loader = Self::new();
        let cli_overrides = CliOverrides::from_cli(cli);
        loader.loader.load_config(None, Some(&cli_overrides))
    }

    /// Load configuration with automatic file detection
    /// Looks for config files in the following order:
    /// 1. config.toml
    /// 2. config.json
    /// 3. index-maker.toml
    /// 4. index-maker.json
    pub fn auto_load(cli: &crate::Cli) -> Result<ApplicationConfig, ConfigBuildError> {
        let loader = Self::new();
        let cli_overrides = CliOverrides::from_cli(cli);

        // Try to find a configuration file automatically
        let config_file = Self::find_config_file(&cli.config_path);

        loader.loader.load_config(config_file.as_ref(), Some(&cli_overrides))
    }

    /// Find configuration file in the given directory
    fn find_config_file(config_dir: &Option<String>) -> Option<PathBuf> {
        let base_dir = config_dir
            .as_ref()
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("configs"));

        let candidates = [
            "config.toml",
            "config.json",
            "index-maker.toml",
            "index-maker.json",
            "application.toml",
            "application.json",
        ];

        for candidate in &candidates {
            let path = base_dir.join(candidate);
            if path.exists() {
                return Some(path);
            }
        }

        None
    }

    /// Validate configuration file exists and is readable
    pub fn validate_config_file<P: Into<PathBuf>>(path: P) -> Result<(), ConfigBuildError> {
        let path = path.into();
        
        if !path.exists() {
            return Err(ConfigBuildError::FileError(format!(
                "Configuration file not found: {}",
                path.display()
            )));
        }

        if !path.is_file() {
            return Err(ConfigBuildError::FileError(format!(
                "Configuration path is not a file: {}",
                path.display()
            )));
        }

        // Check if file is readable
        match std::fs::read_to_string(&path) {
            Ok(_) => Ok(()),
            Err(e) => Err(ConfigBuildError::FileError(format!(
                "Cannot read configuration file {}: {}",
                path.display(),
                e
            ))),
        }
    }

    /// Create a sample configuration file
    pub fn create_sample_config<P: Into<PathBuf>>(
        path: P,
        format: ConfigFormat,
    ) -> Result<(), ConfigBuildError> {
        let path = path.into();
        let config = ApplicationConfig::default();

        let content = match format {
            ConfigFormat::Toml => toml::to_string_pretty(&config)
                .map_err(|e| ConfigBuildError::FileError(format!("TOML serialization error: {}", e)))?,
            ConfigFormat::Json => serde_json::to_string_pretty(&config)
                .map_err(|e| ConfigBuildError::FileError(format!("JSON serialization error: {}", e)))?,
        };

        std::fs::write(&path, content)
            .map_err(|e| ConfigBuildError::FileError(format!(
                "Failed to write sample config to {}: {}",
                path.display(),
                e
            )))?;

        Ok(())
    }

    /// Print configuration as formatted string for debugging
    pub fn print_config(config: &ApplicationConfig, format: ConfigFormat) -> Result<String, ConfigBuildError> {
        match format {
            ConfigFormat::Toml => toml::to_string_pretty(config)
                .map_err(|e| ConfigBuildError::Other(format!("TOML serialization error: {}", e))),
            ConfigFormat::Json => serde_json::to_string_pretty(config)
                .map_err(|e| ConfigBuildError::Other(format!("JSON serialization error: {}", e))),
        }
    }
}

impl Default for ApplicationConfigLoader {
    fn default() -> Self {
        Self::new()
    }
}

/// Configuration file format
#[derive(Debug, Clone, Copy)]
pub enum ConfigFormat {
    Toml,
    Json,
}

impl ConfigFormat {
    /// Detect format from file extension
    pub fn from_path<P: AsRef<std::path::Path>>(path: P) -> Option<Self> {
        path.as_ref()
            .extension()
            .and_then(|ext| ext.to_str())
            .map(|ext| match ext.to_lowercase().as_str() {
                "toml" => ConfigFormat::Toml,
                "json" => ConfigFormat::Json,
                _ => ConfigFormat::Toml, // Default to TOML
            })
    }
}

/// Configuration validation utilities
pub struct ConfigValidator;

impl ConfigValidator {
    /// Validate that all required environment variables are set
    pub fn validate_environment() -> Result<(), ConfigBuildError> {
        let required_vars = [
            // Add required environment variables here
            // "INDEX_MAKER_API_KEY",
            // "INDEX_MAKER_API_SECRET",
        ];

        for var in &required_vars {
            if std::env::var(var).is_err() {
                return Err(ConfigBuildError::EnvError(format!(
                    "Required environment variable {} is not set",
                    var
                )));
            }
        }

        Ok(())
    }

    /// Validate configuration values are within acceptable ranges
    pub fn validate_ranges(config: &ApplicationConfig) -> Result<(), ConfigBuildError> {
        // Validate solver configuration ranges
        if config.solver.max_batch_size > 1000 {
            return Err(ConfigBuildError::ValidationError(
                "max_batch_size cannot exceed 1000".to_string(),
            ));
        }

        if config.solver.max_levels > 100 {
            return Err(ConfigBuildError::ValidationError(
                "max_levels cannot exceed 100".to_string(),
            ));
        }

        // Validate timeout values
        if config.server.timeout_secs > 3600 {
            return Err(ConfigBuildError::ValidationError(
                "server timeout cannot exceed 1 hour".to_string(),
            ));
        }

        if config.market_data.connection_timeout_secs > 300 {
            return Err(ConfigBuildError::ValidationError(
                "market data connection timeout cannot exceed 5 minutes".to_string(),
            ));
        }

        Ok(())
    }

    /// Validate network addresses and URLs
    pub fn validate_network_config(config: &ApplicationConfig) -> Result<(), ConfigBuildError> {
        // Validate bind address format
        if !config.server.bind_address.contains(':') {
            return Err(ConfigBuildError::ValidationError(
                "bind_address must be in format 'host:port'".to_string(),
            ));
        }

        // Validate OTLP URLs if provided
        if let Some(ref url) = config.logging.otlp_trace_url {
            if !url.starts_with("http://") && !url.starts_with("https://") {
                return Err(ConfigBuildError::ValidationError(
                    "otlp_trace_url must be a valid HTTP/HTTPS URL".to_string(),
                ));
            }
        }

        if let Some(ref url) = config.logging.otlp_log_url {
            if !url.starts_with("http://") && !url.starts_with("https://") {
                return Err(ConfigBuildError::ValidationError(
                    "otlp_log_url must be a valid HTTP/HTTPS URL".to_string(),
                ));
            }
        }

        Ok(())
    }

    /// Run all validation checks
    pub fn validate_all(config: &ApplicationConfig) -> Result<(), ConfigBuildError> {
        Self::validate_environment()?;
        Self::validate_ranges(config)?;
        Self::validate_network_config(config)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_format_detection() {
        assert!(matches!(
            ConfigFormat::from_path("config.toml"),
            Some(ConfigFormat::Toml)
        ));
        assert!(matches!(
            ConfigFormat::from_path("config.json"),
            Some(ConfigFormat::Json)
        ));
        assert!(matches!(
            ConfigFormat::from_path("config.unknown"),
            Some(ConfigFormat::Toml)
        )); // Default to TOML
    }

    #[test]
    fn test_default_config_validation() {
        let config = ApplicationConfig::default();
        assert!(ConfigValidator::validate_ranges(&config).is_ok());
        assert!(ConfigValidator::validate_network_config(&config).is_ok());
    }
}
