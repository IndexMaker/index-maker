use std::marker::PhantomData;

use eyre::{Report, Result};
use index_maker::core::bits::Address;

use crate::messages::{FixMessage, FixMessageBuilder, ServerRequest, ServerResponse, SessionId};

pub trait ServerPlugin<R, Q>
where
    R: ServerRequest,
    Q: ServerResponse,
{
    fn process_incoming(&self, message: String, session_id: SessionId) -> Result<R, Report>; // checks - des
    fn process_outgoing(&self, response: &Q) -> Result<String, Report>; // ser - sign
}

pub struct SerdePlugin<R, Q> {
    _phantom_r: PhantomData<R>,
    _phantom_q: PhantomData<Q>,
}

impl<R, Q> SerdePlugin<R, Q> {
    pub fn new() -> Self {
        Self {
            _phantom_r: PhantomData::default(),
            _phantom_q: PhantomData::default(),
        }
    }
}

impl<R, Q> ServerPlugin<R, Q> for SerdePlugin<R, Q>
where
    R: ServerRequest,
    Q: ServerResponse,
{
    fn process_incoming(&self, message: String, session_id: SessionId) -> Result<R, Report> {
        let fix_message = FixMessage(message);
        <R as ServerRequest>::deserialize_from_fix(fix_message, session_id)
    }

    fn process_outgoing(&self, response: &Q) -> Result<String, Report> {
        let builder = FixMessageBuilder::new();
        response.serialize_into_fix(builder).map(|msg| msg.0)
    }
}

pub trait Signer {
    fn get_address(&self) -> &Address;
}
pub struct SignPlugin<R, Q, P> {
    inner_plugin: P,
    _phantom_r: PhantomData<R>,
    _phantom_q: PhantomData<Q>,
}

impl<R, Q, P> SignPlugin<R, Q, P> {
    pub fn new(inner_plugin: P) -> Self {
        Self {
            inner_plugin,
            _phantom_r: PhantomData::default(),
            _phantom_q: PhantomData::default(),
        }
    }
}

impl<R, Q, P> ServerPlugin<R, Q> for SignPlugin<R, Q, P>
where
    R: ServerRequest + Signer,
    Q: ServerResponse,
    P: ServerPlugin<R, Q>,
{
    fn process_incoming(&self, message: String, session_id: SessionId) -> Result<R, Report> {
        // let fix_message = FixMessage(message);
        // MyServerRequest::deserialize_from_fix(fix_message, session_id)
        let de_message = self.inner_plugin.process_incoming(message,  session_id)?;
        de_message.get_address();
        todo!();
    }

    fn process_outgoing(&self, response: &Q) -> Result<String, Report> {
        // let builder = FixMessageBuilder::new();
        // response.serialize_into_fix(builder).map(|msg| msg.0)
        todo!();
    }
}
