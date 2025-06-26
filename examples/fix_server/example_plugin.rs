use eyre::{Report, Result};
use axum_fix_server::{
        messages::{ServerRequest, ServerResponse, SessionId},
        plugins::{
            observer_plugin::ObserverPlugin, seq_num_plugin::{SeqNumPlugin, SeqNumPluginAux}, serde_plugin::SerdePlugin
        },
        server_plugin::ServerPlugin,
    };
use symm_core::core::functional::NotificationHandlerOnce;


// A composite plugin that can wrap other plugins and delegate functionality.
pub struct CompositeServerPlugin<R, Q> 
where
    R: ServerRequest,
    Q: ServerResponse,
{
    observer_plugin: ObserverPlugin<R>,
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
            observer_plugin: ObserverPlugin::new(),
            serde_plugin: SerdePlugin::new(),
            seq_num_plugin: SeqNumPlugin::new(),
        }
    }

    pub fn set_observer_plugin_callback(&mut self, closure: impl NotificationHandlerOnce<R> + 'static) {
        self.observer_plugin.set_observer_closure(closure);
    }
}

impl<R, Q> ServerPlugin<Q> for CompositeServerPlugin<R, Q>
where
    R: ServerRequest + SeqNumPluginAux,
    Q: ServerResponse + SeqNumPluginAux,
{
    fn process_incoming(&self, message: String, session_id: &SessionId) -> Result<()> {
        match self.serde_plugin.process_incoming(message, session_id) {
        Ok(result) => {
            let seq_num = result.get_seq_num(); // Ensure R has this method or adjust accordingly
            if self.seq_num_plugin.valid_seq_num(seq_num, session_id) {
                self.observer_plugin.publish_request(result);
                Ok(())
            } else {
                let error_msg = format!("Invalid sequence number: {}; Last valid: {}", seq_num, self.seq_num_plugin.last_received_seq_num(session_id)); 
                let error_msg = self.process_error(error_msg, session_id)?;
                Err(eyre::eyre!(error_msg))
            }
        }
        Err(e) => {
            let error_msg = self.process_error(e.to_string(), session_id)?;
            return Err(eyre::eyre!(error_msg));
        }
    }
        
    }
    
    fn process_error(&self, error_msg: String, session_id: &SessionId) -> Result<String> {
        let seq_num = self.seq_num_plugin.last_received_seq_num(session_id);
        //let nak: Response = Response::create_nak(session_id, seq_num, error_msg);
        let mut nak = Q::format_errors(session_id, error_msg, seq_num);
        nak.set_seq_num(self.seq_num_plugin.next_seq_num(session_id));
        self.serde_plugin.process_outgoing(nak)
    }

    fn process_outgoing(&self, response: Q) -> Result<String> {
        let mut response = response;
        let session_id = &response.get_session_id();
        response.set_seq_num(self.seq_num_plugin.next_seq_num(session_id));
        self.serde_plugin.process_outgoing(response)
    }

    fn create_session(&self, session_id: &SessionId) -> Result<()> {
        self.seq_num_plugin.create_session(session_id)
    }

    fn destroy_session(&self, session_id: &SessionId) -> Result<()> {
        self.seq_num_plugin.destroy_session(session_id)
    }
    
}