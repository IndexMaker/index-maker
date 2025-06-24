use std::sync::Arc;

use super::config::ConfigBuildError;
use crossbeam::channel::{unbounded, Receiver};
use derive_builder::Builder;
use eyre::Result;
use parking_lot::RwLock;
use rust_decimal::dec;
use symm_core::{
    core::{bits::Amount, functional::IntoObservableSingle},
    order_sender::{
        inventory_manager::{InventoryEvent, InventoryManager},
        order_tracker::OrderTracker,
    },
};

#[derive(Builder)]
#[builder(
    pattern = "owned",
    build_fn(name = "try_build", error = "ConfigBuildError")
)]
pub struct InventoryManagerConfig {
    #[builder(setter(into, strip_option), default)]
    pub zero_threshold: Option<Amount>,
}

impl InventoryManagerConfig {
    #[must_use]
    pub fn builder() -> InventoryManagerConfigBuilder {
        InventoryManagerConfigBuilder::default()
    }

    pub fn make(
        self,
        order_tracker: Arc<RwLock<OrderTracker>>,
    ) -> Result<(Arc<RwLock<InventoryManager>>, Receiver<InventoryEvent>)> {
        let inventory_manager = Arc::new(RwLock::new(InventoryManager::new(
            order_tracker,
            self.zero_threshold.unwrap_or(dec!(0.00001)),
        )));

        let (inventory_event_tx, inventory_event_rx) = unbounded::<InventoryEvent>();

        inventory_manager
            .write()
            .get_single_observer_mut()
            .set_observer_from(inventory_event_tx);

        Ok((inventory_manager, inventory_event_rx))
    }
}
