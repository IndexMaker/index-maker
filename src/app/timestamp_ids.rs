use std::sync::Arc;

use parking_lot::RwLock;
use symm_core::core::bits::{BatchOrderId, OrderId, PaymentId};

use super::config::ConfigBuildError;
use crate::{app::timestamp_ids::util::make_timestamp_id, solver::solver::OrderIdProvider};
use derive_builder::Builder;

pub mod util {
    use chrono::Utc;

    pub fn make_timestamp_id<T>(prefix: &str) -> T
    where
        T: From<String>,
    {
        T::from(format!("{}{}", prefix, Utc::now().timestamp_millis()))
    }
}

pub struct TimestampOrderIds {}

impl TimestampOrderIds {
    pub fn new() -> Self {
        Self {}
    }
}

impl OrderIdProvider for TimestampOrderIds {
    fn next_order_id(&mut self) -> OrderId {
        make_timestamp_id("O-")
    }

    fn next_batch_order_id(&mut self) -> BatchOrderId {
        make_timestamp_id("B-")
    }

    fn next_payment_id(&mut self) -> PaymentId {
        make_timestamp_id("P-")
    }
}

pub enum OrderIdProviderKind {
    Timestamp,
}

#[derive(Builder)]
#[builder(
    pattern = "owned",
    build_fn(name = "try_build", error = "ConfigBuildError")
)]

pub struct OrderIdConfig {
    #[builder(setter(into, strip_option))]
    pub provider_kind: OrderIdProviderKind,

    #[builder(setter(skip))]
    pub(crate) order_id_provider: Option<Arc<RwLock<dyn OrderIdProvider + Send + Sync>>>,
}

impl OrderIdConfig {
    #[must_use]
    pub fn builder() -> OrderIdConfigBuilder {
        OrderIdConfigBuilder::default()
    }
}

impl OrderIdConfigBuilder {
    pub fn build(self) -> Result<OrderIdConfig, ConfigBuildError> {
        let mut config = self.try_build()?;

        config
            .order_id_provider
            .replace(match config.provider_kind {
                OrderIdProviderKind::Timestamp => Arc::new(RwLock::new(TimestampOrderIds::new())),
            });

        Ok(config)
    }
}
