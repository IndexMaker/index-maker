use std::sync::Arc;

use super::config::ConfigBuildError;
use derive_builder::Builder;
use eyre::Result;
use parking_lot::RwLock;

use crate::index::basket_manager::BasketManager;

#[derive(Clone, Builder)]
#[builder(
    pattern = "owned",
    build_fn(name = "try_build", error = "ConfigBuildError")
)]
pub struct BasketManagerConfig {
    #[builder(setter(skip))]
    pub(crate) basket_manager: Option<Arc<RwLock<BasketManager>>>,
}

impl BasketManagerConfig {
    #[must_use]
    pub fn builder() -> BasketManagerConfigBuilder {
        BasketManagerConfigBuilder::default()
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
