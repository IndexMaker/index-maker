use std::sync::{Arc, RwLock as ComponentLock};

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use eyre::{OptionExt, Result};

use super::config::ConfigBuildError;
use derive_builder::Builder;
use symm_core::core::{
    bits::{Address, Amount, Symbol},
    functional::{
        IntoObservableSingleVTable, NotificationHandlerOnce, PublishSingle, SingleObserver,
    },
};

use index_core::{
    blockchain::chain_connector::{ChainConnector, ChainNotification},
    index::basket::Basket,
};

use crate::app::solver::ChainConnectorConfig;

pub struct SimpleChainConnector {
    observer: SingleObserver<ChainNotification>,
}

impl SimpleChainConnector {
    pub fn new() -> Self {
        Self {
            observer: SingleObserver::new(),
        }
    }

    pub fn publish_event(&self, event: ChainNotification) {
        self.observer.publish_single(event);
    }
}

impl IntoObservableSingleVTable<ChainNotification> for SimpleChainConnector {
    fn set_observer(&mut self, observer: Box<dyn NotificationHandlerOnce<ChainNotification>>) {
        self.observer.set_observer(observer);
    }
}

impl ChainConnector for SimpleChainConnector {
    fn solver_weights_set(&self, symbol: Symbol, basket: Arc<Basket>) {
        tracing::info!("SolverWeightsSet: {}", symbol);
    }

    fn mint_index(
        &self,
        chain_id: u32,
        symbol: Symbol,
        quantity: Amount,
        receipient: Address,
        execution_price: Amount,
        execution_time: DateTime<Utc>,
    ) {
        tracing::info!(
            "MintIndex: {} {:0.5} {:0.5} {}",
            symbol,
            quantity,
            execution_price,
            execution_time
        )
    }

    fn burn_index(
        &self,
        chain_id: u32,
        symbol: Symbol,
        quantity: Amount,
        receipient: symm_core::core::bits::Address,
    ) {
        todo!()
    }

    fn withdraw(
        &self,
        chain_id: u32,
        receipient: symm_core::core::bits::Address,
        amount: Amount,
        execution_price: Amount,
        execution_time: chrono::DateTime<chrono::Utc>,
    ) {
        todo!()
    }
}

#[derive(Clone, Builder)]
#[builder(
    pattern = "owned",
    build_fn(name = "try_build", error = "ConfigBuildError")
)]
pub struct SimpleChainConnectorConfig {
    #[builder(setter(skip))]
    simple_chain_connector: Option<Arc<ComponentLock<SimpleChainConnector>>>,
}

impl SimpleChainConnectorConfig {
    #[must_use]
    pub fn builder() -> SimpleChainConnectorConfigBuilder {
        SimpleChainConnectorConfigBuilder::default()
    }

    pub fn expect_chain_connector_cloned(&self) -> Arc<ComponentLock<SimpleChainConnector>> {
        self.simple_chain_connector
            .clone()
            .ok_or(())
            .expect("Failed to get simple chain connector")
    }

    pub fn try_get_chain_connector_cloned(
        &self,
    ) -> Result<Arc<ComponentLock<SimpleChainConnector>>> {
        self.simple_chain_connector
            .clone()
            .ok_or_eyre("Failed to get simple chain connector")
    }
}

impl ChainConnectorConfig for SimpleChainConnectorConfig {
    fn expect_chain_connector_cloned(
        &self,
    ) -> Arc<ComponentLock<dyn ChainConnector + Send + Sync>> {
        self.expect_chain_connector_cloned()
    }

    fn try_get_chain_connector_cloned(
        &self,
    ) -> Result<Arc<ComponentLock<dyn ChainConnector + Send + Sync>>> {
        self.try_get_chain_connector_cloned()
            .map(|x| x as Arc<ComponentLock<dyn ChainConnector + Send + Sync>>)
    }
}

impl SimpleChainConnectorConfigBuilder {
    pub fn build_arc(self) -> Result<Arc<SimpleChainConnectorConfig>, ConfigBuildError> {
        let mut config = self.try_build()?;

        config
            .simple_chain_connector
            .replace(Arc::new(ComponentLock::new(SimpleChainConnector::new())));

        Ok(Arc::new(config))
    }
}
