use std::sync::{Arc, RwLock as ComponentLock};

use crate::{
    app::simple_server::ServerConfig, server::server::Server,
    solver::index_quote_manager::QuoteRequestManager,
};

use super::config::ConfigBuildError;
use derive_builder::Builder;
use eyre::Result;

#[derive(Builder)]
#[builder(
    pattern = "owned",
    build_fn(name = "try_build", error = "ConfigBuildError")
)]
pub struct QuoteRequestManagerConfig {
    #[builder(setter(into, strip_option))]
    pub with_server: ServerConfig,

    #[builder(setter(skip))]
    pub(crate) quote_request_manager: Option<Arc<ComponentLock<QuoteRequestManager>>>,
}

impl QuoteRequestManagerConfig {
    #[must_use]
    pub fn builder() -> QuoteRequestManagerConfigBuilder {
        QuoteRequestManagerConfigBuilder::default()
    }
}

impl QuoteRequestManagerConfigBuilder {
    pub fn build(self) -> Result<QuoteRequestManagerConfig, ConfigBuildError> {
        let mut config = self.try_build()?;

        let server = config
            .with_server
            .server
            .clone()
            .ok_or_else(|| ConfigBuildError::UninitializedField("with_server"))?;

        let quote_request_manager = Arc::new(ComponentLock::new(QuoteRequestManager::new(server)));

        config.quote_request_manager.replace(quote_request_manager);

        Ok(config)
    }
}
