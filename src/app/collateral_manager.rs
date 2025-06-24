use std::sync::{Arc, RwLock as ComponentLock};

use crate::collateral::{
    collateral_manager::CollateralManager, collateral_router::CollateralRouter,
};

use super::config::ConfigBuildError;
use derive_builder::Builder;
use eyre::Result;
use rust_decimal::dec;
use symm_core::core::bits::Amount;

#[derive(Clone, Builder)]
#[builder(
    pattern = "owned",
    build_fn(name = "try_build", error = "ConfigBuildError")
)]
pub struct CollateralManagerConfig {
    #[builder(setter(into, strip_option), default)]
    pub zero_threshold: Option<Amount>,

    #[builder(setter(into, strip_option), default)]
    pub with_router: Option<Arc<ComponentLock<CollateralRouter>>>,

    #[builder(setter(skip))]
    pub(crate) collateral_manager: Option<Arc<ComponentLock<CollateralManager>>>,

    #[builder(setter(skip))]
    pub(crate) collateral_router: Option<Arc<ComponentLock<CollateralRouter>>>,
}

impl CollateralManagerConfig {
    #[must_use]
    pub fn builder() -> CollateralManagerConfigBuilder {
        CollateralManagerConfigBuilder::default()
    }
}

impl CollateralManagerConfigBuilder {
    pub fn build(self) -> Result<CollateralManagerConfig, ConfigBuildError> {
        let mut config = self.try_build()?;

        let collateral_router = config
            .with_router
            .take()
            .unwrap_or_else(|| Arc::new(ComponentLock::new(CollateralRouter::new())));

        config.collateral_router.replace(collateral_router.clone());

        let collateral_manager = Arc::new(ComponentLock::new(CollateralManager::new(
            collateral_router,
            config.zero_threshold.unwrap_or(dec!(0.00001)),
        )));

        config.collateral_manager.replace(collateral_manager);

        Ok(config)
    }
}
