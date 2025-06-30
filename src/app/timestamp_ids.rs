use std::sync::Arc;

use chrono::Utc;
use eyre::{OptionExt, Result};
use parking_lot::RwLock;
use symm_core::core::bits::{BatchOrderId, OrderId, PaymentId};

use super::config::ConfigBuildError;
use crate::{
    app::solver::OrderIdProviderConfig,
    solver::solver::OrderIdProvider,
};
use derive_builder::Builder;

pub struct TimestampOrderIds {
    last_id: i64,
}

impl TimestampOrderIds {
    pub fn new() -> Self {
        Self { last_id: 0 }
    }

    pub fn make_timestamp_id<T>(&mut self, prefix: &str) -> T
    where
        T: From<String>,
    {
        let mut timestamp = Utc::now().timestamp_millis();
        if timestamp == self.last_id {
            timestamp = self.last_id + 1;
        }
        self.last_id = timestamp;

        T::from(format!("{}{}", prefix, self.last_id))
    }
}

impl OrderIdProvider for TimestampOrderIds {
    fn next_order_id(&mut self) -> OrderId {
        self.make_timestamp_id("O-")
    }

    fn next_batch_order_id(&mut self) -> BatchOrderId {
        self.make_timestamp_id("B-")
    }

    fn next_payment_id(&mut self) -> PaymentId {
        self.make_timestamp_id("P-")
    }
}

#[derive(Builder)]
#[builder(
    pattern = "owned",
    build_fn(name = "try_build", error = "ConfigBuildError")
)]

pub struct TimestampOrderIdsConfig {
    #[builder(setter(skip))]
    timestamp_ids: Option<Arc<RwLock<TimestampOrderIds>>>,
}

impl TimestampOrderIdsConfig {
    #[must_use]
    pub fn builder() -> TimestampOrderIdsConfigBuilder {
        TimestampOrderIdsConfigBuilder::default()
    }

    pub fn expect_timestamp_order_ids_cloned(&self) -> Arc<RwLock<TimestampOrderIds>> {
        self.timestamp_ids
            .clone()
            .ok_or(())
            .expect("Failed to get timestamp order ids")
    }

    pub fn try_get_timestamp_order_ids_cloned(&self) -> Result<Arc<RwLock<TimestampOrderIds>>> {
        self.timestamp_ids
            .clone()
            .ok_or_eyre("Failed to get timestamp order ids")
    }
}

impl OrderIdProviderConfig for TimestampOrderIdsConfig {
    fn expect_order_id_provider_cloned(&self) -> Arc<RwLock<dyn OrderIdProvider + Send + Sync>> {
        self.expect_timestamp_order_ids_cloned()
    }

    fn try_get_order_id_provider_cloned(
        &self,
    ) -> Result<Arc<RwLock<dyn OrderIdProvider + Send + Sync>>> {
        self.try_get_timestamp_order_ids_cloned()
            .map(|x| x as Arc<RwLock<dyn OrderIdProvider + Send + Sync>>)
    }
}

impl TimestampOrderIdsConfigBuilder {
    pub fn build_arc(self) -> Result<Arc<TimestampOrderIdsConfig>, ConfigBuildError> {
        let mut config = self.try_build()?;

        config
            .timestamp_ids
            .replace(Arc::new(RwLock::new(TimestampOrderIds::new())));

        Ok(Arc::new(config))
    }
}
