use std::{collections::HashSet, fs, path::Path, sync::Arc};

use super::config::ConfigBuildError;
use derive_builder::Builder;
use eyre::{eyre, Context, OptionExt, Result};
use itertools::Itertools;
use parking_lot::RwLock;
use symm_core::core::{bits::Symbol, json_file_async::read_from_json_file_async};

use index_core::index::{basket::Basket, basket_manager::BasketManager};

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
    index_symbols: Vec<Symbol>,

    #[builder(setter(skip))]
    underlying_asset_symbols: Vec<Symbol>,
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

    pub fn get_index_symbols(&self) -> Vec<Symbol> {
        self.index_symbols.clone()
    }

    pub fn get_underlying_asset_symbols(&self) -> Vec<Symbol> {
        self.underlying_asset_symbols.clone()
    }

    pub async fn load_config_file(&self) -> Result<Vec<(Symbol, String)>> {
        let config_path = self
            .with_config_file
            .as_ref()
            .ok_or_eyre("Config file must be specified")?;

        let json_data = read_from_json_file_async::<serde_json::Value>(config_path)
            .await
            .context("Failed to load basket configuration")?;

        let index_files = json_data
            .get("index_files")
            .and_then(|v| v.as_array())
            .ok_or_eyre("Configuration must contain index_files")?;

        let (index_configs, bad): (Vec<_>, Vec<_>) = index_files
            .into_iter()
            .map(|x| x.as_object().ok_or_eyre("Entry must be an object"))
            .map_ok(|x| {
                x.iter().map(|(k, v)| -> eyre::Result<(Symbol, String)> {
                    Ok((
                        Symbol::from(k.as_str()),
                        String::from(v.as_str().ok_or_eyre("Value must be path string")?),
                    ))
                })
            })
            .flatten_ok()
            .flatten_ok()
            .partition_result();

        if !bad.is_empty() {
            Err(eyre!("Failed to read index configs: {:?}", bad))?;
        }

        Ok(index_configs)
    }
}

impl BasketManagerConfigBuilder {
    pub async fn build(self) -> Result<BasketManagerConfig, ConfigBuildError> {
        let mut config = self.try_build()?;

        config
            .basket_manager
            .replace(Arc::new(RwLock::new(BasketManager::new())));

        if config.with_config_file.is_some() {
            let indexes: Vec<(Symbol, Basket)> = config
                .load_config_file()
                .await
                .map_err(|e| ConfigBuildError::Other(format!("Config load error: {}", e)))?
                .into_iter()
                .map(|(index_symbol, index_path_str)| {
                    let index_path = Path::new(&index_path_str);
                    let content = fs::read_to_string(&index_path)
                        .expect(format!("Failed to read file: {}", index_path_str).as_str());
                    let basket: Basket =
                        serde_json::from_str(&content).expect("Invalid index data");
                    (index_symbol.clone(), basket)
                })
                .collect_vec();

            if indexes.is_empty() {
                tracing::error!("No index loaded, application cannot proceed without indices.");
                std::process::exit(1);
            }

            let mut index_symbols = Vec::new();
            let mut unique_symbols: HashSet<Symbol> = HashSet::new();

            for (index_symbol, basket) in indexes {
                let symbols = basket
                    .basket_assets
                    .iter()
                    .map(|aw| aw.weight.asset.ticker.clone())
                    .collect_vec();

                unique_symbols.extend(symbols);
                config
                    .basket_manager
                    .as_ref()
                    .unwrap()
                    .write()
                    .set_basket(&index_symbol, &Arc::new(basket));

                index_symbols.push(index_symbol);
            }

            config.index_symbols = index_symbols;
            config.underlying_asset_symbols = unique_symbols.into_iter().collect::<Vec<_>>();
        }

        Ok(config)
    }
}
