use std::collections::HashMap;
use eyre::{Report, Result};
use axum_fix_server::messages::{FixMessage, FixMessageBuilder, SessionId};
use axum_fix_server::server_plugin::ServerPlugin;

// Assuming these are the traits for request and response as used in axum-fix-server
use crate::requests::Request; // Adjust based on actual type in fix_server
use crate::responses::Response; // Adjust based on actual type in fix_server

/// A composite plugin that holds a map of other server plugins and delegates processing to them.
pub struct CompositePlugin<R, Q> {
    plugins: HashMap<String, Box<dyn ServerPlugin<R, Q> + Send + Sync>>,
}

impl<R, Q> CompositePlugin<R, Q>
where
    R: ServerRequest,
    Q: ServerResponse,
{
    pub fn new() -> Self {
        Self {
            plugins: HashMap::new(),
        }
    }

    /// Adds a plugin to the composite plugin map with a given name.
    pub fn add_plugin<P>(&mut self, name: &str, plugin: P)
    where
        P: ServerPlugin<R, Q> + Send + Sync + 'static,
    {
        self.plugins.insert(name.to_string(), Box::new(plugin));
    }

    /// Helper to get the default plugin for delegation (e.g., first plugin or based on some logic).
    fn get_default_plugin(&self) -> Option<&dyn ServerPlugin<R, Q>> {
        self.plugins.values().next().map(|p| p.as_ref())
    }
}

impl<R, Q> ServerPlugin<R, Q> for CompositePlugin<R, Q>
where
    R: ServerRequest + Send + 'static,
    Q: ServerResponse + Send + 'static,
{
    fn process_incoming(&self, message: String, session_id: SessionId) -> Result<R, Report> {
        // For simplicity, delegate to the first plugin in the map.
        // In a real implementation, you might select a plugin based on message content or other criteria.
        if let Some(plugin) = self.get_default_plugin() {
            plugin.process_incoming(message, session_id)
        } else {
            Err(eyre::eyre!("No plugins registered in CompositePlugin"))
        }
    }

    fn process_outgoing(&self, response: &Q) -> Result<String, Report> {
        // Similarly, delegate to the first plugin for outgoing messages.
        if let Some(plugin) = self.get_default_plugin() {
            plugin.process_outgoing(response)
        } else {
            Err(eyre::eyre!("No plugins registered in CompositePlugin"))
        }
    }
}

// Placeholder traits to match the expected structure in axum-fix-server
pub trait ServerRequest {
    fn deserialize_from_fix(message: FixMessage, session_id: SessionId) -> Result<Self, Report>
    where
        Self: Sized;
}

pub trait ServerResponse {
    fn serialize_into_fix(&self, builder: FixMessageBuilder) -> Result<FixMessage, Report>;
    fn get_session_id(&self) -> &SessionId;
}