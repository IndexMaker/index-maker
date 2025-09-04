use std::sync::Arc;

use crate::solver::mint_invoice_manager::MintInvoiceManager;

use super::config::ConfigBuildError;
use derive_builder::Builder;
use parking_lot::RwLock;

use eyre::{OptionExt, Result};
use symm_core::core::persistence::util::JsonFilePersistence;

#[derive(Clone, Builder)]
#[builder(
    pattern = "owned",
    build_fn(name = "try_build", error = "ConfigBuildError")
)]
pub struct MintInvoiceManagerConfig {
    #[builder(setter(skip))]
    invoice_manager: Option<Arc<RwLock<MintInvoiceManager>>>,
}

impl MintInvoiceManagerConfig {
    pub fn builder() -> MintInvoiceManagerConfigBuilder {
        MintInvoiceManagerConfigBuilder::default()
    }

    pub fn expect_invoice_manager_cloned(&self) -> Arc<RwLock<MintInvoiceManager>> {
        self.invoice_manager
            .clone()
            .ok_or(())
            .expect("Failed to get router")
    }

    pub fn try_get_collateral_invoice_manager_cloned(
        &self,
    ) -> Result<Arc<RwLock<MintInvoiceManager>>> {
        self.invoice_manager
            .clone()
            .ok_or_eyre("Failed to get collateral invoice_manager")
    }
}

impl MintInvoiceManagerConfigBuilder {
    pub fn build(self) -> Result<MintInvoiceManagerConfig, ConfigBuildError> {
        let mut config = self.try_build()?;

        // TODO: Configure me!
        let persistence = Arc::new(JsonFilePersistence::new(String::from(
            "./persistence/InvoiceManager.json",
        )));

        config
            .invoice_manager
            .replace(Arc::new(RwLock::new(MintInvoiceManager::new(
                persistence
            ))));

        Ok(config)
    }
}
