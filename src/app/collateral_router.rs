use std::sync::{Arc, RwLock as ComponentLock};

use super::config::ConfigBuildError;
use derive_builder::Builder;

use eyre::{OptionExt, Result};
use index_core::collateral::collateral_router::CollateralRouter;

#[derive(Clone, Builder)]
#[builder(
    pattern = "owned",
    build_fn(name = "try_build", error = "ConfigBuildError")
)]
pub struct CollateralRouterConfig {
    #[builder(setter(skip))]
    router: Option<Arc<ComponentLock<CollateralRouter>>>,
}

impl CollateralRouterConfig {
    #[must_use]
    pub fn builder() -> CollateralRouterConfigBuilder {
        CollateralRouterConfigBuilder::default()
    }

    pub fn expect_router_cloned(&self) -> Arc<ComponentLock<CollateralRouter>> {
        self.router.clone().ok_or(()).expect("Failed to get router")
    }

    pub fn try_get_collateral_router_cloned(&self) -> Result<Arc<ComponentLock<CollateralRouter>>> {
        self.router
            .clone()
            .ok_or_eyre("Failed to get collateral router")
    }
}

impl CollateralRouterConfigBuilder {
    pub fn build(self) -> Result<CollateralRouterConfig, ConfigBuildError> {
        let mut config = self.try_build()?;

        config
            .router
            .replace(Arc::new(ComponentLock::new(CollateralRouter::new())));

        Ok(config)
    }
}
