use std::sync::{Arc, RwLock as ComponentLock};

use crate::{app::solver::ServerConfig, solver::index_order_manager::IndexOrderManager};

use super::config::ConfigBuildError;
use derive_builder::Builder;
use eyre::{OptionExt, Result};
use rust_decimal::dec;
use symm_core::core::bits::Amount;

#[derive(Builder)]
#[builder(
    pattern = "owned",
    build_fn(name = "try_build", error = "ConfigBuildError")
)]
pub struct IndexOrderManagerConfig {
    #[builder(setter(into, strip_option), default)]
    pub zero_threshold: Option<Amount>,

    #[builder(setter(into, strip_option))]
    pub with_server: Arc<dyn ServerConfig + Send + Sync>,

    #[builder(setter(skip))]
    index_order_manager: Option<Arc<ComponentLock<IndexOrderManager>>>,
}

impl IndexOrderManagerConfig {
    #[must_use]
    pub fn builder() -> IndexOrderManagerConfigBuilder {
        IndexOrderManagerConfigBuilder::default()
    }

    pub fn expect_index_order_manager_cloned(&self) -> Arc<ComponentLock<IndexOrderManager>> {
        self.index_order_manager.clone().ok_or(()).expect("Failed to get index order manager")
    }

    pub fn try_get_index_order_manager_cloned(&self) -> Result<Arc<ComponentLock<IndexOrderManager>>> {
        self.index_order_manager.clone().ok_or_eyre("Failed to get index order manager")
    }
}

impl IndexOrderManagerConfigBuilder {
    pub fn build(self) -> Result<IndexOrderManagerConfig, ConfigBuildError> {
        let mut config = self.try_build()?;

        let server = config
            .with_server
            .try_get_server_cloned()
            .map_err(|_| ConfigBuildError::UninitializedField("with_server"))?;

        let index_order_manager = Arc::new(ComponentLock::new(IndexOrderManager::new(
            server,
            config.zero_threshold.unwrap_or(dec!(0.00001)),
        )));

        config.index_order_manager.replace(index_order_manager);

        Ok(config)
    }
}
