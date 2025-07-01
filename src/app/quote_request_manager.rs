use std::sync::{Arc, RwLock as ComponentLock};

use crate::{
    app::solver::ServerConfig,
    solver::index_quote_manager::QuoteRequestManager,
};

use super::config::ConfigBuildError;
use derive_builder::Builder;
use eyre::{OptionExt, Result};

#[derive(Builder)]
#[builder(
    pattern = "owned",
    build_fn(name = "try_build", error = "ConfigBuildError")
)]
pub struct QuoteRequestManagerConfig {
    #[builder(setter(into, strip_option))]
    pub with_server: Arc<dyn ServerConfig + Send + Sync>,

    #[builder(setter(skip))]
    quote_request_manager: Option<Arc<ComponentLock<QuoteRequestManager>>>,
}

impl QuoteRequestManagerConfig {
    #[must_use]
    pub fn builder() -> QuoteRequestManagerConfigBuilder {
        QuoteRequestManagerConfigBuilder::default()
    }
    
    pub fn expect_quote_request_manager_cloned(&self) -> Arc<ComponentLock<QuoteRequestManager>> {
        self.quote_request_manager.clone().ok_or(()).expect("Failed to get quote request manager")
    }

    pub fn try_get_quote_request_manager_cloned(&self) -> Result<Arc<ComponentLock<QuoteRequestManager>>> {
        self.quote_request_manager.clone().ok_or_eyre("Failed to get quote request manager")
    }
}

impl QuoteRequestManagerConfigBuilder {
    pub fn build(self) -> Result<QuoteRequestManagerConfig, ConfigBuildError> {
        let mut config = self.try_build()?;

        let server = config
            .with_server
            .try_get_server_cloned()
            .map_err(|_| ConfigBuildError::UninitializedField("with_server"))?;

        let quote_request_manager = Arc::new(ComponentLock::new(QuoteRequestManager::new(server)));

        config.quote_request_manager.replace(quote_request_manager);

        Ok(config)
    }
}
