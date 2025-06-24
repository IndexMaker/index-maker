use std::sync::Arc;

use super::config::ConfigBuildError;
use binance_order_sending::{binance_order_sending::BinanceOrderSending, credentials::Credentials};
use crossbeam::channel::{unbounded, Receiver};
use derive_builder::Builder;
use eyre::{eyre, OptionExt, Result};
use parking_lot::RwLock;
use symm_core::{
    core::functional::IntoObservableSingleArc,
    order_sender::order_connector::OrderConnectorNotification,
};

#[derive(Builder)]
#[builder(
    pattern = "owned",
    build_fn(name = "try_build", error = "ConfigBuildError")
)]
pub struct OrderSenderConfig {
    #[builder(setter(into, strip_option), default)]
    pub credentials: Vec<Credentials>,
}

impl OrderSenderConfig {
    #[must_use]
    pub fn builder() -> OrderSenderConfigBuilder {
        OrderSenderConfigBuilder::default()
    }

    pub fn make(self,
    ) -> Result<(
        Arc<RwLock<BinanceOrderSending>>,
        Receiver<OrderConnectorNotification>,
    )> {
        let order_sender = Arc::new(RwLock::new(BinanceOrderSending::new()));

        let (connector_event_tx, connector_event_rx) = unbounded::<OrderConnectorNotification>();

        order_sender
            .write()
            .get_single_observer_arc()
            .write()
            .set_observer_from(connector_event_tx);

        order_sender
            .write()
            .start()
            .map_err(|err| eyre!("Failed to start order sender: {:?}", err))?;

        order_sender
            .write()
            .logon(self.credentials)
            .map_err(|err| eyre!("Failed to logon: {:?}", err))?;

        Ok((order_sender, connector_event_rx))
    }
}
