use std::{fs, path::Path, sync::Arc};

use super::config::ConfigBuildError;
use derive_builder::Builder;
use eyre::{OptionExt, Result};
use itertools::Itertools;
use parking_lot::RwLock;
use symm_core::core::bits::Symbol;

use crate::index::{basket::Basket, basket_manager::BasketManager};

#[derive(Clone, Builder)]
#[builder(
    pattern = "owned",
    build_fn(name = "try_build", error = "ConfigBuildError")
)]
pub struct BasketManagerConfig {
    #[builder(setter(skip))]
    basket_manager: Option<Arc<RwLock<BasketManager>>>,

    #[builder(setter(into, strip_option), default)]
    with_config_file: Option<String>,

    #[builder(setter(skip))]
    symbols: Vec<Symbol>,
    //asset_manager:
}

impl BasketManagerConfig {
    #[must_use]
    pub fn builder() -> BasketManagerConfigBuilder {
        BasketManagerConfigBuilder::default()
    }

    pub fn expect_basket_manager_cloned(&self) -> Arc<RwLock<BasketManager>> {
        self.basket_manager
            .clone()
            .ok_or(())
            .expect("Failed to get basket manager")
    }

    pub fn try_get_basket_manager_cloned(&self) -> Result<Arc<RwLock<BasketManager>>> {
        self.basket_manager
            .clone()
            .ok_or_eyre("Failed to get basket manager")
    }

    pub fn get_symbols(&self) -> Vec<Symbol> {
        self.symbols.clone()
    }


    pub fn load_config_file(&self) -> Result<Vec<(Symbol, String)>> {
        let Some(path_str) = &self.with_config_file else {
            return Err(eyre::eyre!("No config file path provided"));
        };
        let config_path = Path::new(path_str.as_str());

        let mut indexes_configs: Vec<(Symbol, String)> = Vec::new();
        if config_path.exists() {
            let content = fs::read_to_string(config_path)
                .expect("Failed to read BasketManagerConfig.json");
            let json_data: serde_json::Value = serde_json::from_str(&content)
                .expect("Failed to parse BasketManagerConfig.json");
            let indexes_files = json_data.get("indexes_files").and_then(|v| v.as_array()).ok_or_eyre("No 'indexes_files' array found in config file.")?;
            
            for index_obj in indexes_files {
                if let Some(obj) = index_obj.as_object() {
                    for (index_name, file_path) in obj {
                        if let Some(path_str) = file_path.as_str() {
                            let index_symbol = Symbol::from(index_name.as_str());
                            indexes_configs.push((index_symbol, path_str.to_string()));
                        }
                    }
                }
            }
        } else {
            return Err(eyre::eyre!("BasketManagerConfig.json config file not found at: {}", path_str));
        }
        Ok(indexes_configs)
    }
}

impl BasketManagerConfigBuilder {
    pub fn build(self) -> Result<BasketManagerConfig, ConfigBuildError> {
        let mut config = self.try_build()?;

        config
            .basket_manager
            .replace(Arc::new(RwLock::new(BasketManager::new())));

        if config.with_config_file.is_some() {
            let indexes: Vec<(Symbol, Basket)> = config.load_config_file()
                .map_err(|e| ConfigBuildError::Other(format!("Config load error: {}", e)))?
                .into_iter()
                .map(|(index_symbol, index_path_str)| {
                    let index_path = Path::new(&index_path_str);
                    let content = fs::read_to_string(&index_path)
                        .expect(format!("Failed to read file: {}", index_path_str).as_str());
                    let basket: Basket = serde_json::from_str(&content)
                        .expect("Invalid index data");
                    (index_symbol.clone(), basket)
                })
                .collect_vec();

            if indexes.is_empty() {
                tracing::error!("No index loaded, application cannot proceed without indices.");
                std::process::exit(1);
            }

            let mut unique_symbols: Vec<Symbol> = Vec::new();
            for (index_symbol, basket) in indexes {
                let symbols = basket
                    .basket_assets
                    .iter()
                    .map(|aw| aw.weight.asset.ticker.clone())
                    .filter(|s| !unique_symbols.contains(s))
                    .collect_vec();

                unique_symbols.extend(symbols);
                config
                    .basket_manager
                    .as_ref()
                    .unwrap()
                    .write()
                    .set_basket(&index_symbol, &Arc::new(basket));
            }
            config.symbols = unique_symbols;
        }

        Ok(config)
    }
}
