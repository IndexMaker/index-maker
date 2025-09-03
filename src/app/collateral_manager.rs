use std::sync::{Arc, RwLock as ComponentLock};

use crate::{
    app::collateral_router::CollateralRouterConfig,
    collateral::collateral_manager::CollateralManager,
};

use super::config::ConfigBuildError;
use derive_builder::Builder;
use eyre::{OptionExt, Result};
use rust_decimal::dec;
use symm_core::core::{bits::Amount, persistence::util::JsonFilePersistence};

#[derive(Clone, Builder)]
#[builder(
    pattern = "owned",
    build_fn(name = "try_build", error = "ConfigBuildError")
)]
pub struct CollateralManagerConfig {
    #[builder(setter(into, strip_option), default)]
    pub zero_threshold: Option<Amount>,

    #[builder(setter(into, strip_option))]
    pub with_router: CollateralRouterConfig,

    #[builder(setter(skip))]
    collateral_manager: Option<Arc<ComponentLock<CollateralManager>>>,
}

impl CollateralManagerConfig {
    #[must_use]
    pub fn builder() -> CollateralManagerConfigBuilder {
        CollateralManagerConfigBuilder::default()
    }

    pub fn expect_collateral_manager_cloned(&self) -> Arc<ComponentLock<CollateralManager>> {
        self.collateral_manager
            .clone()
            .ok_or(())
            .expect("Failed to get collateral manager")
    }

    pub fn try_get_collateral_manager_cloned(
        &self,
    ) -> Result<Arc<ComponentLock<CollateralManager>>> {
        self.collateral_manager
            .clone()
            .ok_or_eyre("Failed to get collateral manager")
    }
}

impl CollateralManagerConfigBuilder {
    pub fn build(self) -> Result<CollateralManagerConfig, ConfigBuildError> {
        let mut config = self.try_build()?;

        let collateral_router = config.with_router.expect_router_cloned();

        // TODO: Configure me!
        let persistence = Arc::new(JsonFilePersistence::new(String::from(
            "./persistence/CollateralManager.json",
        )));

        let collateral_manager = Arc::new(ComponentLock::new(CollateralManager::new(
            collateral_router,
            persistence,
            config.zero_threshold.unwrap_or(dec!(0.00001)),
        )));

        config.collateral_manager.replace(collateral_manager);

        Ok(config)
    }
}
