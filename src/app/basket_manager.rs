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
    assets_file_path: String,

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
}

impl BasketManagerConfigBuilder {
    pub fn build(self) -> Result<BasketManagerConfig, ConfigBuildError> {
        let mut config = self.try_build()?;

        config
            .basket_manager
            .replace(Arc::new(RwLock::new(BasketManager::new())));

        let mut indexes: Vec<(Symbol, Basket)> = Vec::new();

        let path_str = config.assets_file_path.clone();
        let indexes_path = Path::new(path_str.as_str());

        if indexes_path.exists() && indexes_path.is_dir() {
            for entry in fs::read_dir(indexes_path).expect("Failed to read indexes directory") {
                let entry = entry.expect("Failed to read directory entry");
                let file_name = entry.file_name().into_string().expect("Invalid file name");
                if file_name.ends_with("_Latest_rebalance.json") {
                    let index_name = file_name.split('_').next().unwrap_or("UNKNOWN").to_string();
                    let index_symbol = Symbol::from(index_name.as_str());
                    let content =
                        fs::read_to_string(entry.path()).expect("Failed to read JSON file");

                    let basket: Basket =
                        serde_json::from_str(content.as_str()).expect("Invalid index data");
                    indexes.push((index_symbol.clone(), basket));
                }
            }
        } else {
            tracing::warn!("Indexes directory does not exist.")
        }

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

        Ok(config)
    }
}
