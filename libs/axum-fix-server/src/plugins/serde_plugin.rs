use std::marker::PhantomData;
use eyre::{Report, Result};
use crate::{messages::{FixMessage, ServerRequest, ServerResponse, SessionId}};

pub struct SerdePlugin<R, Q> {
    _phantom_r: PhantomData<R>,
    _phantom_q: PhantomData<Q>,
}


impl<R, Q> SerdePlugin<R, Q>
where 
    R: ServerRequest,
    Q: ServerResponse,
{
    pub fn new() -> Self {
        Self {
            _phantom_r: PhantomData::default(),
            _phantom_q: PhantomData::default(),
        }
    }

    pub fn process_incoming(&self, message: String, session_id: &SessionId) -> Result<R> {
        let fix_message = FixMessage(message);
        <R as ServerRequest>::deserialize_from_fix(fix_message, session_id)
    }

    pub fn process_outgoing(&self, response: Q) -> Result<String, Report> {
        response.serialize_into_fix().map(|msg| msg.0)
    }
}