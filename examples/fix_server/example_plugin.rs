use eyre::{Report, Result};
use axum_fix_server::{
        messages::{ServerRequest, ServerResponse, SessionId},
        plugins::{
            seq_num_plugin::{SeqNumPlugin, SeqNumPluginAux},
            serde_plugin::SerdePlugin,
        },
        server_plugin::ServerPlugin,
    };

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
    fn process_incoming(&self, message: String, session_id: SessionId) -> Result<R, Report> {
        let result = self.serde_plugin.process_incoming(message.clone(), session_id.clone())?;
        let seq_num = result.get_seq_num(); // Ensure R has this method or adjust accordingly
        if self.seq_num_plugin.valid_seq_num(seq_num, session_id.clone()) {
            Ok(result)
        } else {
            Err(eyre::eyre!("Invalid sequence number: {}", seq_num))
        }
    }

    fn process_outgoing(&self, response: &mut Q) -> Result<String, Report> {
        let session_id = response.get_session_id();
        response.set_seq_num(self.seq_num_plugin.next_seq_num(session_id));
        self.serde_plugin.process_outgoing(response)
    }

    fn create_session(&self, session_id: SessionId) -> Result<(), Report> {
        self.seq_num_plugin.create_session(session_id)
    }

    fn destroy_session(&self, session_id: SessionId) -> Result<(), Report> {
        self.seq_num_plugin.destroy_session(session_id)
    }
}