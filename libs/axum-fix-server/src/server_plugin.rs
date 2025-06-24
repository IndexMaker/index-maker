use eyre::{Report, Result};
use crate::messages::{ServerRequest, ServerResponse, SessionId};

/// Base trait for server plugins, defining core functionality for processing messages
/// and managing sessions.
pub trait ServerPlugin<R, Q>
where
    R: ServerRequest,
    Q: ServerResponse,
{
    fn process_incoming(&self, message: String, session_id: &SessionId) -> Result<R, Report>;

    fn process_error(&self, error_msg: String, session_id: &SessionId) -> Result<String>;

    fn process_outgoing(&self, response: Q) -> Result<String, Report>;

    fn create_session(&self, session_id: &SessionId) -> Result<(), Report>;

    fn destroy_session(&self, session_id: &SessionId) -> Result<(), Report>;
}