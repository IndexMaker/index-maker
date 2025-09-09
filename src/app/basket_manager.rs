use std::{collections::HashSet, fs, path::Path, sync::Arc};

use super::config::{BasketManagerConfigData, ConfigBuildError};
use derive_builder::Builder;
use eyre::{eyre, Context, OptionExt, Result};
use itertools::Itertools;
use parking_lot::RwLock;
use symm_core::core::{bits::Symbol, json_file_async::read_from_json_file_async};

use index_core::index::{basket::Basket, basket_manager::BasketManager};

/// Loaded basket data with metadata
/// Separates the concern of loading JSON files from runtime management
pub struct LoadedBasket {
    /// The symbol/identifier for this basket
    pub symbol: Symbol,
    /// The loaded basket data
    pub basket: Basket,
    /// Source file path for debugging and logging
    pub source_file: String,
}

/// JSON file loader for basket configurations
///
/// This struct handles all the file I/O and parsing concerns separately from runtime management.
/// It provides a clean separation between configuration loading and runtime basket management.
///
/// # Design Philosophy
///
/// - **Single Responsibility**: Only handles JSON file loading and parsing
/// - **Error Handling**: Provides detailed error messages with file paths and symbols
/// - **Path Resolution**: Supports both absolute and relative paths with optional base path
/// - **Validation**: Ensures loaded baskets contain valid data before returning
///
/// # Usage
///
/// ```rust
/// let loader = BasketJsonLoader::new();
/// let baskets = loader.load_baskets_from_config(&config_data)?;
/// ```
pub struct BasketJsonLoader {
    /// Base path for resolving relative file paths
    base_path: Option<String>,
}

/// BasketManagerConfig following index-maker builder pattern
/// This holds the ACTUAL RUNTIME INSTANCE, not configuration
#[derive(Builder)]
pub struct BasketManagerConfig {
    /// The basket manager instance (runtime state)
    #[builder(setter(skip))]
    basket_manager: Arc<RwLock<BasketManager>>,
    /// All unique symbols from loaded baskets (computed result)
    symbols: Vec<Symbol>,
    /// Loaded baskets for reference and debugging
    loaded_baskets: Vec<LoadedBasket>,
    /// Optional config file and derived fields (for legacy builder paths)
    #[builder(setter(into, strip_option), default)]
    with_config_file: Option<String>,
    #[builder(setter(skip), default)]
    index_symbols: Vec<Symbol>,
    #[builder(setter(skip), default)]
    underlying_asset_symbols: Vec<Symbol>,
}

impl BasketJsonLoader {
    /// Create a new BasketJsonLoader
    pub fn new() -> Self {
        Self { base_path: None }
    }

    /// Create a new BasketJsonLoader with a base path for resolving relative paths
    pub fn with_base_path<P: Into<String>>(base_path: P) -> Self {
        Self {
            base_path: Some(base_path.into()),
        }
    }

    /// Load baskets from configuration data
    /// This method handles all the JSON file I/O and parsing concerns
    pub fn load_baskets_from_config(
        &self,
        config_data: &BasketManagerConfigData,
    ) -> Result<Vec<LoadedBasket>, ConfigBuildError> {
        tracing::info!("Loading baskets from configuration data");

        if config_data.index_files.is_empty() {
            return Err(ConfigBuildError::ValidationError(
                "No basket files configured".to_string(),
            ));
        }

        let loaded_baskets: Result<Vec<LoadedBasket>, ConfigBuildError> = config_data
            .index_files
            .iter()
            .map(|mapping| self.load_single_basket(&mapping.symbol, &mapping.file_path))
            .collect();

        let baskets = loaded_baskets?;

        tracing::info!(
            count = baskets.len(),
            "Successfully loaded all baskets from configuration"
        );

        Ok(baskets)
    }

    /// Load a single basket from a file path
    fn load_single_basket(
        &self,
        symbol: &Symbol,
        file_path: &str,
    ) -> Result<LoadedBasket, ConfigBuildError> {
        let resolved_path = self.resolve_path(file_path);
        let path = Path::new(&resolved_path);

        tracing::debug!(
            symbol = %symbol,
            file_path = %resolved_path,
            "Loading basket from file"
        );

        // Read file content
        let content = fs::read_to_string(path).map_err(|e| {
            ConfigBuildError::Other(format!(
                "Failed to read basket file '{}' for symbol '{}': {:?}",
                resolved_path, symbol, e
            ))
        })?;

        // Parse JSON content
        let basket: Basket = serde_json::from_str(&content).map_err(|e| {
            ConfigBuildError::Other(format!(
                "Invalid JSON in basket file '{}' for symbol '{}': {:?}",
                resolved_path, symbol, e
            ))
        })?;

        // Validate basket has assets
        if basket.basket_assets.is_empty() {
            return Err(ConfigBuildError::ValidationError(format!(
                "Basket '{}' from file '{}' contains no assets",
                symbol, resolved_path
            )));
        }

        tracing::debug!(
            symbol = %symbol,
            assets_count = basket.basket_assets.len(),
            "Successfully loaded basket"
        );

        Ok(LoadedBasket {
            symbol: symbol.clone(),
            basket,
            source_file: resolved_path,
        })
    }

    /// Resolve file path, considering base path if set
    fn resolve_path(&self, file_path: &str) -> String {
        match &self.base_path {
            Some(base) => {
                let path = Path::new(base).join(file_path);
                path.to_string_lossy().to_string()
            }
            None => file_path.to_string(),
        }
    }
}

impl BasketManagerConfig {
    /// Create BasketManagerConfig from ApplicationConfig data
    /// This is a FACTORY METHOD that creates the runtime instance
    pub fn from_config_data(
        config_data: &BasketManagerConfigData,
    ) -> Result<Self, ConfigBuildError> {
        tracing::info!("Creating BasketManagerConfig from configuration data");

        // Use the dedicated loader for clean separation of concerns
        let loader = BasketJsonLoader::new();
        let loaded_baskets = loader.load_baskets_from_config(config_data)?;

        if loaded_baskets.is_empty() {
            return Err(ConfigBuildError::ValidationError(
                "No baskets loaded, application cannot proceed without indices".to_string(),
            ));
        }

        // Create the runtime basket manager
        let basket_manager = Arc::new(RwLock::new(BasketManager::new()));

        // Populate the basket manager and collect symbols using iterators
        let (unique_symbols, processed_baskets): (HashSet<Symbol>, Vec<LoadedBasket>) =
            loaded_baskets
                .into_iter()
                .map(|loaded_basket| {
                    // Collect symbols from this basket
                    let basket_symbols: HashSet<Symbol> = loaded_basket
                        .basket
                        .basket_assets
                        .iter()
                        .map(|aw| aw.weight.asset.ticker.clone())
                        .collect();

                    // Add basket to the manager (move the basket instead of cloning)
                    let symbol = loaded_basket.symbol.clone();
                    let source_file = loaded_basket.source_file.clone();
                    basket_manager
                        .write()
                        .set_basket(&symbol, &Arc::new(loaded_basket.basket));

                    // Return symbols and processed basket metadata
                    (
                        basket_symbols,
                        LoadedBasket {
                            symbol,
                            basket: Basket::default(), // Use default for metadata storage
                            source_file,
                        },
                    )
                })
                .fold(
                    (HashSet::new(), Vec::new()),
                    |(mut all_symbols, mut baskets), (symbols, basket)| {
                        all_symbols.extend(symbols);
                        baskets.push(basket);
                        (all_symbols, baskets)
                    },
                );

        let config = Self {
            basket_manager,
            symbols: unique_symbols.into_iter().collect(),
            loaded_baskets: processed_baskets,
        };

        tracing::info!(
            baskets_count = config.loaded_baskets.len(),
            symbols_count = config.symbols.len(),
            "BasketManagerConfig created successfully from configuration data"
        );

        Ok(config)
    }

    /// Create BasketManagerConfig with a custom base path for file resolution
    pub fn from_config_data_with_base_path(
        config_data: &BasketManagerConfigData,
        base_path: &str,
    ) -> Result<Self, ConfigBuildError> {
        tracing::info!(
            base_path = %base_path,
            "Creating BasketManagerConfig with custom base path"
        );

        // Use the dedicated loader with base path for clean separation of concerns
        let loader = BasketJsonLoader::with_base_path(base_path);
        let loaded_baskets = loader.load_baskets_from_config(config_data)?;

        if loaded_baskets.is_empty() {
            return Err(ConfigBuildError::ValidationError(
                "No baskets loaded, application cannot proceed without indices".to_string(),
            ));
        }

        // Create the runtime basket manager
        let basket_manager = Arc::new(RwLock::new(BasketManager::new()));

        // Populate the basket manager and collect symbols using iterators
        let (unique_symbols, processed_baskets): (HashSet<Symbol>, Vec<LoadedBasket>) =
            loaded_baskets
                .into_iter()
                .map(|loaded_basket| {
                    // Collect symbols from this basket
                    let basket_symbols: HashSet<Symbol> = loaded_basket
                        .basket
                        .basket_assets
                        .iter()
                        .map(|aw| aw.weight.asset.ticker.clone())
                        .collect();

                    // Add basket to the manager (move the basket instead of cloning)
                    let symbol = loaded_basket.symbol.clone();
                    let source_file = loaded_basket.source_file.clone();
                    basket_manager
                        .write()
                        .set_basket(&symbol, &Arc::new(loaded_basket.basket));

                    // Return symbols and processed basket metadata
                    (
                        basket_symbols,
                        LoadedBasket {
                            symbol,
                            basket: Basket::default(), // Use default for metadata storage
                            source_file,
                        },
                    )
                })
                .fold(
                    (HashSet::new(), Vec::new()),
                    |(mut all_symbols, mut baskets), (symbols, basket)| {
                        all_symbols.extend(symbols);
                        baskets.push(basket);
                        (all_symbols, baskets)
                    },
                );

        let config = Self {
            basket_manager,
            symbols: unique_symbols.into_iter().collect(),
            loaded_baskets: processed_baskets,
        };

        tracing::info!(
            baskets_count = config.loaded_baskets.len(),
            symbols_count = config.symbols.len(),
            base_path = %base_path,
            "BasketManagerConfig created successfully with custom base path"
        );

        Ok(config)
    }

    /// Get the basket manager instance (runtime access)
    pub fn expect_basket_manager_cloned(&self) -> Arc<RwLock<BasketManager>> {
        self.basket_manager.clone()
    }

    /// Try to get the basket manager instance with error handling
    pub fn try_get_basket_manager_cloned(&self) -> Result<Arc<RwLock<BasketManager>>> {
        Ok(self.basket_manager.clone())
    }

    /// Get all unique symbols from loaded baskets
    pub fn get_symbols(&self) -> Vec<Symbol> {
        self.symbols.clone()
    }

    /// Backward-compatible getters for legacy call sites
    pub fn get_index_symbols(&self) -> Vec<Symbol> {
        self.symbols.clone()
    }

    pub fn get_underlying_asset_symbols(&self) -> Vec<Symbol> {
        self.symbols.clone()
    }

    /// Get loaded baskets for debugging and inspection
    pub fn get_loaded_baskets(&self) -> &[LoadedBasket] {
        &self.loaded_baskets
    }

    /// Get basket count
    pub fn basket_count(&self) -> usize {
        self.loaded_baskets.len()
    }

    /// Get symbol count
    pub fn symbol_count(&self) -> usize {
        self.symbols.len()
    }
}

impl BasketManagerConfigBuilder {
    pub async fn build(self) -> Result<BasketManagerConfig, ConfigBuildError> {
        let mut cfg = self
            .try_build()
            .map_err(|e| ConfigBuildError::Other(format!("Build error: {}", e)))?;

        // Ensure basket_manager exists
        if Arc::strong_count(&cfg.basket_manager) == 0 {
            cfg.basket_manager = Arc::new(RwLock::new(BasketManager::new()));
        }

        // If config file is specified, load and populate
        if let Some(path_str) = cfg.with_config_file.clone() {
            let json = fs::read_to_string(Path::new(&path_str)).map_err(|e| {
                ConfigBuildError::Other(format!("Failed to read file {}: {}", path_str, e))
            })?;
            let value: serde_json::Value = serde_json::from_str(&json).map_err(|e| {
                ConfigBuildError::Other(format!("Invalid JSON in {}: {}", path_str, e))
            })?;
            let index_files = value
                .get("index_files")
                .and_then(|v| v.as_array())
                .ok_or_else(|| {
                    ConfigBuildError::ValidationError(
                        "Configuration must contain index_files".to_string(),
                    )
                })?;

            let mut unique_symbols: HashSet<Symbol> = HashSet::new();
            for entry in index_files {
                let obj = entry.as_object().ok_or_else(|| {
                    ConfigBuildError::ValidationError("Entry must be an object".to_string())
                })?;
                for (k, v) in obj.iter() {
                    let p = v.as_str().ok_or_else(|| {
                        ConfigBuildError::ValidationError("Value must be path string".to_string())
                    })?;
                    let content = fs::read_to_string(Path::new(p)).map_err(|e| {
                        ConfigBuildError::Other(format!("Failed to read file {}: {}", p, e))
                    })?;
                    let basket: Basket = serde_json::from_str(&content).map_err(|e| {
                        ConfigBuildError::Other(format!("Invalid index data in {}: {}", p, e))
                    })?;
                    unique_symbols.extend(
                        basket
                            .basket_assets
                            .iter()
                            .map(|aw| aw.weight.asset.ticker.clone()),
                    );
                    cfg.basket_manager
                        .write()
                        .set_basket(&Symbol::from(k.as_str()), &Arc::new(basket));
                }
            }
            cfg.symbols = unique_symbols.into_iter().collect();
        }

        Ok(cfg)
    }
}
