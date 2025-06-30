use std::collections::HashSet;

use eyre::{Result};
use crate::messages::{ServerResponse, SessionId};

/// Base trait for server plugins, defining core functionality for processing messages
/// and managing sessions.
pub trait ServerPlugin<Q>
where
    Q: ServerResponse,
{
    /// process_incoming
    /// 
    /// Consumes the incoming message and returns Ok or Err. The plugin implementation 
    /// is responsible for validating the buffer, deserializing and publushing
    /// the resulting request to the application.  
    fn process_incoming(&self, message: String, session_id: &SessionId) -> Result<()>;

    /// process_outgoing
    /// 
    /// Consumes the incoming response of `ServerResponse` type and returns Err or 
    /// a HashSet of buffers and the sessions they must be sent. 
    fn process_outgoing(&self, response: Q) -> Result<HashSet<(SessionId, String)>>;

    /// create_session
    /// 
    /// Notifies the plugin of session creation, allowing
    /// for implementation methods. Returns Ok or Err.
    fn create_session(&self, session_id: &SessionId) -> Result<()>;

    /// destroy_session
    /// 
    /// Notifies the plugin of session destruction, allowing
    /// for implementation methods. Returns Ok or Err.
    fn destroy_session(&self, session_id: &SessionId) -> Result<()>;
}