use eyre::{Report, Result};
use axum_fix_server::{
        messages::{ServerRequest, ServerResponse, SessionId},
        plugins::{
            seq_num_plugin::{SeqNumPlugin, SeqNumPluginAux},
            serde_plugin::SerdePlugin,
        },
        server_plugin::ServerPlugin,
    };

use crate::responses::{self, Response};

// A composite plugin that can wrap other plugins and delegate functionality.
pub struct CompositeServerPlugin<R, Q> 
where
    R: ServerRequest,
    Q: ServerResponse,
{
    serde_plugin: SerdePlugin<R, Q>,
    seq_num_plugin: SeqNumPlugin<R, Q>,
}

impl<R, Q> CompositeServerPlugin<R, Q> 
where
    R: ServerRequest + SeqNumPluginAux,
    Q: ServerResponse + SeqNumPluginAux,
{
    pub fn new() -> Self {
        Self {
            serde_plugin: SerdePlugin::new(),
            seq_num_plugin: SeqNumPlugin::new(),
        }
    }
}

impl<R, Q> ServerPlugin<R, Q> for CompositeServerPlugin<R, Q>
where
    R: ServerRequest + SeqNumPluginAux,
    Q: ServerResponse + SeqNumPluginAux,
{
    fn process_incoming(&self, message: String, session_id: &SessionId) -> Result<R, Report> {
        let result = self.serde_plugin.process_incoming(message, session_id)?;
        let seq_num = result.get_seq_num(); // Ensure R has this method or adjust accordingly
        if self.seq_num_plugin.valid_seq_num(seq_num, session_id) {
            Ok(result)
        } else {
            Err(eyre::eyre!("Invalid sequence number: {}; Last valid: {}", seq_num, self.seq_num_plugin.last_received_seq_num(session_id)))
        }
    }
    
    fn process_error(&self, error_msg: String, session_id: &SessionId) -> Result<String> {
        let seq_num = self.seq_num_plugin.last_received_seq_num(session_id);
        //let nak: Response = Response::create_nak(session_id, seq_num, error_msg);
        let mut nak = Q::format_errors(session_id, error_msg, seq_num);
        nak.set_seq_num(self.seq_num_plugin.next_seq_num(session_id));
        self.serde_plugin.process_outgoing(nak)
    }

    fn process_outgoing(&self, response: Q) -> Result<String, Report> {
        let mut response = response;
        let session_id = &response.get_session_id();
        response.set_seq_num(self.seq_num_plugin.next_seq_num(session_id));
        self.serde_plugin.process_outgoing(response)
    }

    fn create_session(&self, session_id: &SessionId) -> Result<(), Report> {
        self.seq_num_plugin.create_session(session_id)
    }

    fn destroy_session(&self, session_id: &SessionId) -> Result<(), Report> {
        self.seq_num_plugin.destroy_session(session_id)
    }
    
}