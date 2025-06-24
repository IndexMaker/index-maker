use chrono::{TimeDelta, Utc};
use std::sync::{Arc, RwLock as ComponentLock};
use symm_core::core::bits::{BatchOrderId, OrderId, PaymentId};

use crate::solver::{batch_manager::BatchManager, solver::OrderIdProvider};

use super::config::ConfigBuildError;
use derive_builder::Builder;
use eyre::Result;
use rust_decimal::dec;
use symm_core::core::bits::Amount;

pub struct TimestampOrderIds {}

impl OrderIdProvider for TimestampOrderIds {
    fn next_order_id(&mut self) -> OrderId {
        OrderId::from(format!("O-{}", Utc::now().timestamp_millis()))
    }

    fn next_batch_order_id(&mut self) -> BatchOrderId {
        BatchOrderId::from(format!("B-{}", Utc::now().timestamp_millis()))
    }

    fn next_payment_id(&mut self) -> PaymentId {
        PaymentId::from(format!("P-{}", Utc::now().timestamp_millis()))
    }
}

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
    pub(crate) batch_manager: Option<Arc<ComponentLock<BatchManager>>>,
}

impl BatchManagerConfig {
    #[must_use]
    pub fn builder() -> BatchManagerConfigBuilder {
        BatchManagerConfigBuilder::default()
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
