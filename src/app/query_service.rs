use std::sync::{Arc, RwLock as ComponentLock};

use crate::{
    collateral::collateral_manager::CollateralManager,
    query::{query_service::QueryService, query_service_state::QueryServiceState},
    solver::{index_order_manager::IndexOrderManager, mint_invoice_manager::MintInvoiceManager},
};

use super::config::ConfigBuildError;
use derive_builder::Builder;
use eyre::{OptionExt, Result};
use parking_lot::RwLock;
use symm_core::order_sender::inventory_manager::InventoryManager;

#[derive(Clone, Builder)]
#[builder(
    pattern = "owned",
    build_fn(name = "try_build", error = "ConfigBuildError")
)]

pub struct QueryServiceConfig {
    #[builder(setter(into, strip_option), default)]
    pub address: Option<String>,

    #[builder(setter(into, strip_option))]
    pub with_collateral_manager: Arc<ComponentLock<CollateralManager>>,

    #[builder(setter(into, strip_option))]
    pub with_index_order_manager: Arc<ComponentLock<IndexOrderManager>>,

    #[builder(setter(into, strip_option))]
    pub with_inventory_manager: Arc<RwLock<InventoryManager>>,

    #[builder(setter(into, strip_option))]
    pub with_invoice_manager: Arc<RwLock<MintInvoiceManager>>,

    #[builder(setter(skip))]
    pub service: Option<Arc<QueryService>>,
}

impl QueryServiceConfig {
    #[must_use]
    pub fn builder() -> QueryServiceConfigBuilder {
        QueryServiceConfigBuilder::default()
    }

    pub fn expect_service_cloned(&self) -> Arc<QueryService> {
        self.service
            .clone()
            .ok_or(())
            .expect("Failed to get fix service")
    }

    pub fn try_get_service_cloned(&self) -> Result<Arc<QueryService>> {
        self.service
            .clone()
            .ok_or_eyre("Failed to get axum service")
    }

    pub async fn start(&self) -> Result<()> {
        let address = self
            .address
            .clone()
            .unwrap_or(String::from("127.0.0.1:3000"));

        self.service
            .as_ref()
            .ok_or_eyre("service not configured")?
            .start(address)
            .await
    }

    pub async fn stop(&self) -> Result<()> {
        self.service
            .as_ref()
            .ok_or_eyre("service not configured")?
            .stop()
            .await
    }
}

impl QueryServiceConfigBuilder {
    pub fn build(self) -> Result<QueryServiceConfig, ConfigBuildError> {
        let mut config = self.try_build()?;

        let service = QueryService::new(QueryServiceState::new(
            config.with_collateral_manager.clone(),
            config.with_index_order_manager.clone(),
            config.with_inventory_manager.clone(),
            config.with_invoice_manager.clone(),
        ));

        config.service.replace(Arc::new(service));

        Ok(config)
    }
}
