use std::sync::Arc;

use super::config::ConfigBuildError;
use binance_order_sending::{binance_order_sending::BinanceOrderSending, credentials::Credentials};
use derive_builder::Builder;
use eyre::{eyre, OptionExt, Result};
use parking_lot::RwLock;
use rust_decimal::dec;
use symm_core::{
    core::bits::Amount,
    order_sender::{inventory_manager::InventoryManager, order_tracker::OrderTracker},
};

#[derive(Builder)]
#[builder(
    pattern = "owned",
    build_fn(name = "try_build", error = "ConfigBuildError")
)]
pub struct OrderSenderConfig {
    #[builder(setter(into, strip_option), default)]
    pub zero_threshold: Option<Amount>,

    #[builder(setter(into, strip_option), default)]
    pub with_order_tracker: Option<bool>,

    #[builder(setter(into, strip_option), default)]
    pub with_inventory_manager: Option<bool>,

    #[builder(setter(into, strip_option), default)]
    pub credentials: Vec<Credentials>,

    #[builder(setter(skip))]
    order_sender: Option<Arc<RwLock<BinanceOrderSending>>>,

    #[builder(setter(skip))]
    order_tracker: Option<Arc<RwLock<OrderTracker>>>,

    #[builder(setter(skip))]
    inventory_manager: Option<Arc<RwLock<InventoryManager>>>,
}

impl OrderSenderConfig {
    #[must_use]
    pub fn builder() -> OrderSenderConfigBuilder {
        OrderSenderConfigBuilder::default()
    }

    pub fn expect_order_sender_cloned(&self) -> Arc<RwLock<BinanceOrderSending>> {
        self.order_sender.clone().ok_or(()).expect("Failed to get order sender")
    }

    pub fn try_get_order_sender_cloned(&self) -> Result<Arc<RwLock<BinanceOrderSending>>> {
        self.order_sender.clone().ok_or_eyre("Failed to get order sender")
    }

    pub fn expect_order_tracker_cloned(&self) -> Arc<RwLock<OrderTracker>> {
        self.order_tracker.clone().ok_or(()).expect("Failed to get order tracker")
    }

    pub fn try_get_order_tracker_cloned(&self) -> Result<Arc<RwLock<OrderTracker>>> {
        self.order_tracker.clone().ok_or_eyre("Failed to get order tracker")
    }

    pub fn expect_inventory_manager_cloned(&self) -> Arc<RwLock<InventoryManager>> {
        self.inventory_manager.clone().ok_or(()).expect("Failed to get inventory manager")
    }

    pub fn try_get_inventory_manager_cloned(&self) -> Result<Arc<RwLock<InventoryManager>>> {
        self.inventory_manager.clone().ok_or_eyre("Failed to get inventory manager")
    }

    pub fn start(&mut self) -> Result<()> {
        if let Some(order_sender) = &self.order_sender {
            order_sender
                .write()
                .start()
                .map_err(|err| eyre!("Failed to start order sender: {:?}", err))?;

            order_sender
                .write()
                .logon(self.credentials.drain(..))
                .map_err(|err| eyre!("Failed to logon: {:?}", err))?;

            Ok(())
        } else {
            Err(eyre!(""))
        }
    }
}

impl OrderSenderConfigBuilder {
    pub fn build(self) -> Result<OrderSenderConfig, ConfigBuildError> {
        let mut config = self.try_build()?;

        let order_sender = Arc::new(RwLock::new(BinanceOrderSending::new()));
        config.order_sender.replace(order_sender.clone());

        if config.with_order_tracker.unwrap_or(true) {
            let order_tracker = Arc::new(RwLock::new(OrderTracker::new(
                order_sender.clone(),
                config.zero_threshold.unwrap_or(dec!(0.00001)),
            )));

            config.order_tracker.replace(order_tracker.clone());

            if config.with_inventory_manager.unwrap_or(true) {
                config
                    .inventory_manager
                    .replace(Arc::new(RwLock::new(InventoryManager::new(
                        order_tracker,
                        config.zero_threshold.unwrap_or(dec!(0.00001)),
                    ))));
            }
        } else {
            Err(ConfigBuildError::UninitializedField("with_order_tracker"))?;
        }

        Ok(config)
    }
}
