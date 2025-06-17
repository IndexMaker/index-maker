use eyre::{Report, Result};

use crate::messages::{FixMessage, FixMessageBuilder, MyServerRequest, MyServerResponse, ServerRequest, ServerResponse, SessionId};

pub trait ServerPlugin <R, Q>
where
    R: ServerRequest,
    Q: ServerResponse,
{
    fn process_incoming(&self, message: String, session_id: SessionId) -> Result<R, Report>; // checks - des
    fn process_outgoing(&self, response: &dyn ServerResponse) -> Result<String, Report>; // ser - sign
 }

pub struct DummyPlugin;

impl ServerPlugin<MyServerRequest, MyServerResponse> for DummyPlugin {
    fn process_incoming(&self, message: String, session_id: SessionId) -> Result<MyServerRequest, Report> {
        let fix_message = FixMessage(message);
        MyServerRequest::deserialize_from_fix(fix_message, session_id)
    }

    fn process_outgoing(&self, response: &dyn ServerResponse) -> Result<String, Report> {
        let builder = FixMessageBuilder::new();
        response.serialize_into_fix(builder).map(|msg| msg.0)
    }
}
