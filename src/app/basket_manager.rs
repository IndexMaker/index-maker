use std::sync::Arc;

use super::config::ConfigBuildError;
use derive_builder::Builder;
use eyre::{OptionExt, Result};
use parking_lot::RwLock;

use crate::index::basket_manager::BasketManager;

#[derive(Clone, Builder)]
#[builder(
    pattern = "owned",
    build_fn(name = "try_build", error = "ConfigBuildError")
)]
pub struct BasketManagerConfig {
    #[builder(setter(skip))]
    basket_manager: Option<Arc<RwLock<BasketManager>>>,
}

impl BasketManagerConfig {
    #[must_use]
    pub fn builder() -> BasketManagerConfigBuilder {
        BasketManagerConfigBuilder::default()
    }

    pub fn expect_basket_manager_cloned(&self) -> Arc<RwLock<BasketManager>> {
        self.basket_manager.clone().ok_or(()).expect("Failed to get basket manager")
    }

    pub fn try_get_basket_manager_cloned(&self) -> Result<Arc<RwLock<BasketManager>>> {
        self.basket_manager.clone().ok_or_eyre("Failed to get basket manager")
    }
}

impl BasketManagerConfigBuilder {
    pub fn build(self) -> Result<BasketManagerConfig, ConfigBuildError> {
        let mut config = self.try_build()?;

        config
            .basket_manager
            .replace(Arc::new(RwLock::new(BasketManager::new())));

        Ok(config)
    }
}
