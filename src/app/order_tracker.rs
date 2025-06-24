use std::sync::Arc;

use super::config::ConfigBuildError;
use crossbeam::channel::{unbounded, Receiver};
use derive_builder::Builder;
use eyre::{OptionExt, Result};
use parking_lot::RwLock;
use rust_decimal::dec;
use symm_core::{
    core::{bits::Amount, functional::IntoObservableSingle},
    order_sender::{
        order_connector::OrderConnector,
        order_tracker::{OrderTracker, OrderTrackerNotification},
    },
};

#[derive(Builder)]
#[builder(
    pattern = "owned",
    build_fn(name = "try_build", error = "ConfigBuildError")
)]
pub struct OrderTrackerConfig {
    #[builder(setter(into, strip_option), default)]
    pub zero_threshold: Option<Amount>,
}

impl OrderTrackerConfig {
    #[must_use]
    pub fn builder() -> OrderTrackerConfigBuilder {
        OrderTrackerConfigBuilder::default()
    }

    pub fn make(
        self,
        order_connector: Arc<RwLock<dyn OrderConnector>>,
    ) -> Result<(
        Arc<RwLock<OrderTracker>>,
        Receiver<OrderTrackerNotification>,
    )> {
        let order_tracker = Arc::new(RwLock::new(OrderTracker::new(
            order_connector,
            self.zero_threshold.unwrap_or(dec!(0.00001)),
        )));

        let (order_tracker_tx, order_tracker_rx) = unbounded::<OrderTrackerNotification>();

        order_tracker
            .write()
            .get_single_observer_mut()
            .set_observer_from(order_tracker_tx);

        Ok((order_tracker, order_tracker_rx))
    }
}
