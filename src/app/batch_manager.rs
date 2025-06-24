use eyre::{eyre, Result};
use std::sync::{Arc, RwLock as ComponentLock};

use super::config::ConfigBuildError;
use chrono::TimeDelta;
use crossbeam::channel::{unbounded, Receiver};
use derive_builder::Builder;
use rust_decimal::dec;
use symm_core::core::{bits::Amount, functional::IntoObservableSingle};

use crate::solver::batch_manager::{BatchEvent, BatchManager};

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
}

impl BatchManagerConfig {
    #[must_use]
    pub fn builder() -> BatchManagerConfigBuilder {
        BatchManagerConfigBuilder::default()
    }

    pub fn make(self) -> Result<(Arc<ComponentLock<BatchManager>>, Receiver<BatchEvent>)> {
        let batch_manager = Arc::new(ComponentLock::new(BatchManager::new(
            self.max_batch_size.unwrap_or(4),
            self.zero_threshold.unwrap_or(dec!(0.00001)),
            self.fill_threshold.unwrap_or(dec!(0.9999)),
            self.mint_threshold.unwrap_or(dec!(0.99)),
            self.mint_wait_period.unwrap_or(TimeDelta::seconds(10)),
        )));

        let (batch_event_tx, batch_event_rx) = unbounded::<BatchEvent>();

        batch_manager
            .write()
            .map_err(|err| eyre!("Failed to build batch manager: {:?}", err))?
            .get_single_observer_mut()
            .set_observer_from(batch_event_tx);

        Ok((batch_manager, batch_event_rx))
    }
}
