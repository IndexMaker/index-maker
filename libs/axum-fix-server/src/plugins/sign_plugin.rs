use std::marker::PhantomData;
use eyre::{Report, Result};
use crate::{messages::{ServerRequest, ServerResponse, SessionId}, server_plugin::ServerPlugin};

pub trait SignPluginAux {
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
    R: ServerRequest + SignPluginAux,
    Q: ServerResponse,
    P: ServerPlugin<R, Q>,
{
    fn process_incoming(&self, message: String, session_id: SessionId) -> Result<R, Report> {
        let de_message = self.inner_plugin.process_incoming(message, session_id)?;
        de_message.get_address();
        todo!();
    }

    fn process_outgoing(&self, response: &Q) -> Result<String, Report> {
        todo!();
    }
    
    fn create_session(&mut self, session_id: SessionId) -> Result<(), Report> {
        todo!()
    }
    
    fn destroy_session(&mut self, session_id: SessionId) -> Result<(), Report> {
        todo!()
    }
}