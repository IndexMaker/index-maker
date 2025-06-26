use eyre::{Result};
use crate::messages::{ServerResponse, SessionId};

/// Base trait for server plugins, defining core functionality for processing messages
/// and managing sessions.
pub trait ServerPlugin<Q>
where
    Q: ServerResponse,
{
    
    fn process_incoming(&self, message: String, session_id: &SessionId) -> Result<()>;

    fn process_outgoing(&self, response: Q) -> Result<String>;

    fn create_session(&self, session_id: &SessionId) -> Result<()>;

    fn destroy_session(&self, session_id: &SessionId) -> Result<()>;
}