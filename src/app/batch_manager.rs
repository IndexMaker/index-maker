use chrono::TimeDelta;
use std::sync::{Arc, RwLock as ComponentLock};

use crate::solver::batch_manager::BatchManager;

use super::config::ConfigBuildError;
use derive_builder::Builder;
use eyre::{OptionExt, Result};
use rust_decimal::dec;
use symm_core::core::bits::Amount;

#[derive(Clone, Builder)]
#[builder(
    pattern = "owned",
    build_fn(name = "try_build", error = "ConfigBuildError")
)]
pub struct BatchManagerConfig {
    #[builder(setter(into, strip_option), default)]
    pub max_batch_size: Option<usize>,

    #[builder(setter(into, strip_option), default)]
    pub zero_threshold: Option<Amount>,

    #[builder(setter(into, strip_option), default)]
    pub fill_threshold: Option<Amount>,

    #[builder(setter(into, strip_option), default)]
    pub mint_threshold: Option<Amount>,

    #[builder(setter(into, strip_option), default)]
    pub mint_wait_period: Option<TimeDelta>,

    #[builder(setter(skip))]
    batch_manager: Option<Arc<ComponentLock<BatchManager>>>,
}

impl BatchManagerConfig {
    #[must_use]
    pub fn builder() -> BatchManagerConfigBuilder {
        BatchManagerConfigBuilder::default()
    }

    pub fn expect_batch_manager_cloned(&self) -> Arc<ComponentLock<BatchManager>> {
        self.batch_manager
            .clone()
            .ok_or(())
            .expect("Failed to get batch manager")
    }

    pub fn try_get_batch_manager_cloned(&self) -> Result<Arc<ComponentLock<BatchManager>>> {
        self.batch_manager
            .clone()
            .ok_or_eyre("Failed to get batch manager")
    }
}

impl BatchManagerConfigBuilder {
    pub fn build(self) -> Result<BatchManagerConfig, ConfigBuildError> {
        let mut config = self.try_build()?;

        let batch_manager = Arc::new(ComponentLock::new(BatchManager::new(
            config.max_batch_size.unwrap_or(4),
            config.zero_threshold.unwrap_or(dec!(0.00001)),
            config.fill_threshold.unwrap_or(dec!(0.9999)),
            config.mint_threshold.unwrap_or(dec!(0.99)),
            config.mint_wait_period.unwrap_or(TimeDelta::seconds(10)),
        )));

        config.batch_manager.replace(batch_manager);

        Ok(config)
    }
}
