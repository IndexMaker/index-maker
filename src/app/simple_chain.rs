use std::sync::{Arc, RwLock as ComponentLock};

use chrono::{DateTime, Utc};

use super::config::ConfigBuildError;
use derive_builder::Builder;
use symm_core::core::{
    bits::{Address, Amount, Symbol},
    functional::{
        IntoObservableSingleVTable, NotificationHandlerOnce, PublishSingle, SingleObserver,
    },
};

use crate::{
    blockchain::chain_connector::{ChainConnector, ChainNotification},
    index::basket::Basket,
};

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

#[derive(Debug, Clone)]
pub enum ChainConnectorKind {
    Simple,
}

pub enum ChainConnectorHandoffEvent {
    Simple(Arc<ComponentLock<SimpleChainConnector>>),
}

#[derive(Clone, Builder)]
#[builder(
    pattern = "owned",
    build_fn(name = "try_build", error = "ConfigBuildError")
)]
pub struct ChainConnectorConfig {
    #[builder(setter(into, strip_option), default)]
    pub chain_connector_kind: Option<ChainConnectorKind>,

    #[builder(setter(skip))]
    pub(crate) chain_connector: Option<Arc<ComponentLock<dyn ChainConnector + Send + Sync>>>,
}

impl ChainConnectorConfig {
    #[must_use]
    pub fn builder() -> ChainConnectorConfigBuilder {
        ChainConnectorConfigBuilder::default()
    }

    pub fn expect_chain_connector_cloned(
        &self,
    ) -> Arc<ComponentLock<dyn ChainConnector + Send + Sync>> {
        self.chain_connector.clone().ok_or(()).expect("Failed to get server")
    }
}

impl ChainConnectorConfigBuilder {
    pub fn build(
        self,
        handoff: impl NotificationHandlerOnce<ChainConnectorHandoffEvent>,
    ) -> Result<ChainConnectorConfig, ConfigBuildError> {
        let mut config = self.try_build()?;

        config.chain_connector.replace(match config.chain_connector_kind.take() {
            Some(ChainConnectorKind::Simple) => {
                let server = Arc::new(ComponentLock::new(SimpleChainConnector::new()));
                handoff.handle_notification(ChainConnectorHandoffEvent::Simple(server.clone()));
                server
            }
            None => Err(ConfigBuildError::UninitializedField("chain_connector_kind"))?,
        });

        Ok(config)
    }
}
