use std::marker::PhantomData;
use eyre::{Report, Result};
use crate::{messages::{FixMessage, SessionId}};

pub trait ServerRequest
where
    Self: Sized,
{
    fn deserialize_from_fix(message: FixMessage, session_id: SessionId) -> Result<Self, Report>;
}

pub trait ServerResponse {
    // make a trait -> fn get_session_id(&self) ->&SessionId;
    fn serialize_into_fix(&self) -> Result<FixMessage, Report>;
}

pub struct ErrorPlugin<R, Q> {
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

    pub fn deserialize(&self, message: String, session_id: SessionId) -> Result<R> {
        let fix_message = FixMessage(message);
        <R as ServerRequest>::deserialize_from_fix(fix_message, session_id)
    }

    pub fn serialize(&self, response: &Q) -> Result<String> {
        response.serialize_into_fix().map(|msg| msg.0)
    }
}