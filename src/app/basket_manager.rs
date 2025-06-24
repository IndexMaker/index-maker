use std::sync::Arc;

use super::config::ConfigBuildError;
use crossbeam::channel::{unbounded, Receiver};
use derive_builder::Builder;
use eyre::Result;
use parking_lot::RwLock;
use symm_core::core::functional::IntoObservableSingle;

use crate::index::basket_manager::{BasketManager, BasketNotification};

#[derive(Clone, Builder)]
#[builder(
    pattern = "owned",
    build_fn(name = "try_build", error = "ConfigBuildError")
)]
pub struct BasketManagerConfig {}

impl BasketManagerConfig {
    #[must_use]
    pub fn builder() -> BasketManagerConfigBuilder {
        BasketManagerConfigBuilder::default()
    }

    pub fn make(self) -> Result<(Arc<RwLock<BasketManager>>, Receiver<BasketNotification>)> {
        let basket_manager = Arc::new(RwLock::new(BasketManager::new()));

        let (basket_event_tx, basket_event_rx) = unbounded::<BasketNotification>();

        basket_manager
            .write()
            .get_single_observer_mut()
            .set_observer_from(basket_event_tx);

        Ok((basket_manager, basket_event_rx))
    }
}
