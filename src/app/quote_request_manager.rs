use std::sync::{Arc, RwLock as ComponentLock};

use crate::{server::server::Server, solver::index_quote_manager::QuoteRequestManager};

use super::config::ConfigBuildError;
use derive_builder::Builder;
use eyre::Result;
use parking_lot::RwLock;

#[derive(Clone, Builder)]
#[builder(
    pattern = "owned",
    build_fn(name = "try_build", error = "ConfigBuildError")
)]
pub struct QuoteRequestManagerConfig {
    #[builder(setter(into, strip_option), default)]
    pub with_server: Option<Arc<RwLock<dyn Server>>>,

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
            .take()
            .ok_or_else(|| ConfigBuildError::UninitializedField("with_server"))?;

        config.with_server.replace(server.clone());

        let quote_request_manager = Arc::new(ComponentLock::new(QuoteRequestManager::new(server)));

        config.quote_request_manager.replace(quote_request_manager);

        Ok(config)
    }
}
