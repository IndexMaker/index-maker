use std::sync::{Arc, RwLock as ComponentLock};

use crate::{
    server::server::Server,
    solver::index_order_manager::IndexOrderManager,
};

use super::config::ConfigBuildError;
use derive_builder::Builder;
use eyre::Result;
use parking_lot::RwLock;
use rust_decimal::dec;
use symm_core::core::bits::Amount;

#[derive(Clone, Builder)]
#[builder(
    pattern = "owned",
    build_fn(name = "try_build", error = "ConfigBuildError")
)]
pub struct IndexOrderManagerConfig {
    #[builder(setter(into, strip_option), default)]
    pub zero_threshold: Option<Amount>,

    #[builder(setter(into, strip_option), default)]
    pub with_server: Option<Arc<RwLock<dyn Server>>>,

    #[builder(setter(skip))]
    pub(crate) index_order_manager: Option<Arc<ComponentLock<IndexOrderManager>>>,
}

impl IndexOrderManagerConfig {
    #[must_use]
    pub fn builder() -> IndexOrderManagerConfigBuilder {
        IndexOrderManagerConfigBuilder::default()
    }
}

impl IndexOrderManagerConfigBuilder {
    pub fn build(self) -> Result<IndexOrderManagerConfig, ConfigBuildError> {
        let mut config = self.try_build()?;

        let server = config
            .with_server
            .take()
            .ok_or_else(|| ConfigBuildError::UninitializedField("with_server"))?;

        config.with_server.replace(server.clone());

        let index_order_manager = Arc::new(ComponentLock::new(IndexOrderManager::new(
            server,
            config.zero_threshold.unwrap_or(dec!(0.00001)),
        )));

        config.index_order_manager.replace(index_order_manager);

        Ok(config)
    }
}
