use std::sync::Arc;

use super::config::ConfigBuildError;
use binance_order_sending::{
    binance_order_sending::BinanceOrderSending,
    credentials::{self, Credentials},
};
use chrono::Utc;
use derive_builder::Builder;
use eyre::{eyre, OptionExt, Result};
use itertools::Itertools;
use parking_lot::RwLock;
use rust_decimal::dec;
use symm_core::{
    core::{
        self,
        bits::{self, Amount, SingleOrder, Symbol},
        functional::{
            self, IntoObservableSingleVTable, NotificationHandlerOnce, PublishSingle,
            SingleObserver,
        },
    },
    order_sender::{
        self,
        inventory_manager::InventoryManager,
        order_connector::{self, OrderConnector, OrderConnectorNotification, SessionId},
        order_tracker::OrderTracker,
        position::LotId,
    },
};

struct SimpleOrderSender {
    observer: SingleObserver<OrderConnectorNotification>,
}

impl SimpleOrderSender {
    pub fn new() -> Self {
        Self {
            observer: SingleObserver::new(),
        }
    }

    pub fn logon(&self) {
        self.observer
            .publish_single(OrderConnectorNotification::SessionLogon {
                session_id: "Session-1".into(),
                timestamp: Utc::now(),
            });
    }

    pub fn stop(&self) {
        self.observer
            .publish_single(OrderConnectorNotification::SessionLogout {
                session_id: "Session-1".into(),
                reason: "Session ended".into(),
                timestamp: Utc::now(),
            });
    }
}

impl OrderConnector for SimpleOrderSender {
    fn send_order(&mut self, _session_id: SessionId, order: &Arc<SingleOrder>) -> Result<()> {
        self.observer
            .publish_single(OrderConnectorNotification::NewOrder {
                order_id: order.order_id.clone(),
                symbol: order.symbol.clone(),
                side: order.side,
                price: order.price,
                quantity: order.quantity,
                timestamp: order.created_timestamp,
            });

        self.observer
            .publish_single(OrderConnectorNotification::Fill {
                order_id: order.order_id.clone(),
                symbol: order.symbol.clone(),
                side: order.side,
                price: order.price,
                quantity: order.quantity,
                timestamp: order.created_timestamp,
                lot_id: format!("{}-L1", order.order_id).into(),
                fee: Amount::ZERO,
            });

        self.observer
            .publish_single(OrderConnectorNotification::Cancel {
                order_id: order.order_id.clone(),
                symbol: order.symbol.clone(),
                side: order.side,
                quantity: Amount::ZERO,
                timestamp: order.created_timestamp,
            });

        Ok(())
    }
}

#[derive(Default)]
pub enum OrderSenderCredentials {
    #[default]
    Simple,
    Binance(Vec<Credentials>),
}

enum OrderSenderVariant {
    Simple(Arc<RwLock<SimpleOrderSender>>),
    Binance(Arc<RwLock<BinanceOrderSending>>, Vec<Credentials>),
}

impl OrderSenderVariant {
    fn get_order_connector(&self) -> Arc<RwLock<dyn OrderConnector + Send + Sync>> {
        match self {
            OrderSenderVariant::Simple(inner) => inner.clone(),
            OrderSenderVariant::Binance(inner, _) => {
                inner.clone() as Arc<RwLock<dyn OrderConnector + Send + Sync>>
            }
        }
    }
}

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
    pub credentials: OrderSenderCredentials,

    #[builder(setter(into, strip_option), default)]
    pub symbols: Vec<Symbol>,

    #[builder(setter(skip))]
    order_sender: Option<OrderSenderVariant>,

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

    pub fn expect_order_sender_cloned(&self) -> Arc<RwLock<dyn OrderConnector + Send + Sync>> {
        self.order_sender
            .as_ref()
            .ok_or(())
            .expect("Failed to get order sender")
            .get_order_connector()
    }

    pub fn try_get_order_sender_cloned(
        &self,
    ) -> Result<Arc<RwLock<dyn OrderConnector + Send + Sync>>> {
        let order_sender_variant = self
            .order_sender
            .as_ref()
            .ok_or_eyre("Failed to get order sender")?;

        Ok(order_sender_variant.get_order_connector())
    }

    pub fn expect_order_tracker_cloned(&self) -> Arc<RwLock<OrderTracker>> {
        self.order_tracker
            .clone()
            .ok_or(())
            .expect("Failed to get order tracker")
    }

    pub fn try_get_order_tracker_cloned(&self) -> Result<Arc<RwLock<OrderTracker>>> {
        self.order_tracker
            .clone()
            .ok_or_eyre("Failed to get order tracker")
    }

    pub fn expect_inventory_manager_cloned(&self) -> Arc<RwLock<InventoryManager>> {
        self.inventory_manager
            .clone()
            .ok_or(())
            .expect("Failed to get inventory manager")
    }

    pub fn try_get_inventory_manager_cloned(&self) -> Result<Arc<RwLock<InventoryManager>>> {
        self.inventory_manager
            .clone()
            .ok_or_eyre("Failed to get inventory manager")
    }

    pub fn start(&mut self) -> Result<()> {
        if let Some(order_sender) = &mut self.order_sender {
            match order_sender {
                OrderSenderVariant::Simple(order_sender) => {
                    order_sender.read().logon();
                }
                OrderSenderVariant::Binance(order_sender, credentials) => {
                    order_sender
                        .write()
                        .start(self.symbols.clone())
                        .map_err(|err| eyre!("Failed to start order sender: {:?}", err))?;

                    order_sender
                        .write()
                        .logon(credentials.drain(..))
                        .map_err(|err| eyre!("Failed to logon: {:?}", err))?;
                }
            }
            Ok(())
        } else {
            Err(eyre!(""))
        }
    }

    pub async fn stop(&mut self) -> Result<()> {
        if let Some(order_sender) = &mut self.order_sender {
            match order_sender {
                OrderSenderVariant::Simple(order_sender) => {
                    order_sender.read().stop();
                }
                OrderSenderVariant::Binance(order_sender, _) => {
                    order_sender.write().stop().await?;
                }
            }
        }
        Ok(())
    }
}

impl IntoObservableSingleVTable<OrderConnectorNotification> for SimpleOrderSender {
    fn set_observer(
        &mut self,
        observer: Box<dyn NotificationHandlerOnce<OrderConnectorNotification>>,
    ) {
        self.observer.set_observer(observer);
    }
}

impl OrderSenderConfigBuilder {
    pub fn build(self) -> Result<OrderSenderConfig, ConfigBuildError> {
        let mut config = self.try_build()?;

        let order_sender_variant = match &mut config.credentials {
            OrderSenderCredentials::Simple => {
                OrderSenderVariant::Simple(Arc::new(RwLock::new(SimpleOrderSender::new())))
            }
            OrderSenderCredentials::Binance(credentials) => OrderSenderVariant::Binance(
                Arc::new(RwLock::new(BinanceOrderSending::new())),
                credentials.drain(..).collect_vec(),
            ),
        };

        let order_sender = order_sender_variant.get_order_connector();

        config.order_sender.replace(order_sender_variant);

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
