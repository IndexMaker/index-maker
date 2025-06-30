use std::collections::HashSet;

use alloy::primitives::address;
use eyre::Result;
use axum_fix_server::{
        messages::{ServerRequest, ServerResponse, SessionId},
        plugins::{
            observer_plugin::ObserverPlugin, seq_num_plugin::{SeqNumPlugin, WithSeqNumPlugin}, serde_plugin::SerdePlugin, user_plugin::{UserPlugin, WithUserPlugin}
        },
        server_plugin::ServerPlugin,
    };
use symm_core::core::{bits::Address, functional::NotificationHandlerOnce};


// A composite plugin that can wrap other plugins and delegate functionality.
pub struct ExamplePlugin<R, Q> 
where
    R: ServerRequest,
    Q: ServerResponse,
{
    observer_plugin: ObserverPlugin<R>,
    serde_plugin: SerdePlugin<R, Q>,
    seq_num_plugin: SeqNumPlugin<R, Q>,
    user_plugin: UserPlugin,
}

impl<R, Q> ExamplePlugin<R, Q> 
where
    R: ServerRequest + WithSeqNumPlugin,
    Q: ServerResponse + WithSeqNumPlugin,
{
    pub fn new() -> Self {
        Self {
            observer_plugin: ObserverPlugin::new(),
            serde_plugin: SerdePlugin::new(),
            seq_num_plugin: SeqNumPlugin::new(),
            user_plugin: UserPlugin::new(),
        }
    }

    pub fn set_observer_plugin_callback(&mut self, closure: impl NotificationHandlerOnce<R> + 'static) {
        self.observer_plugin.set_observer_closure(closure);
    }

    fn process_error(&self, user_id: &(u32, Address), error_msg: String, session_id: &SessionId) -> Result<String> {
        let seq_num = self.seq_num_plugin.last_received_seq_num(session_id);
        //let nak: Response = Response::create_nak(session_id, seq_num, error_msg);
        let mut nak = Q::format_errors(&user_id, session_id, error_msg, seq_num);
        nak.set_seq_num(self.seq_num_plugin.next_seq_num(session_id));
        self.serde_plugin.process_outgoing(nak)
    }
}

impl<R, Q> ServerPlugin<Q> for ExamplePlugin<R, Q>
where
    R: ServerRequest + WithSeqNumPlugin + WithUserPlugin,
    Q: ServerResponse + WithSeqNumPlugin + WithUserPlugin + Clone,
{
    fn process_incoming(&self, message: String, session_id: &SessionId) -> Result<()> {
        match self.serde_plugin.process_incoming(message, session_id) {
            Ok(result) => {
                let user_id = &result.get_user_id();
                self.user_plugin.add_add_user_session(&user_id, session_id);

                let seq_num = result.get_seq_num(); // Ensure R has this method or adjust accordingly
                if self.seq_num_plugin.valid_seq_num(seq_num, session_id) {
                    self.observer_plugin.publish_request(result);
                    Ok(())
                } else {
                    let error_msg = format!("Invalid sequence number: {}; Last valid: {}", seq_num, self.seq_num_plugin.last_received_seq_num(session_id)); 
                    let error_msg = self.process_error(user_id, error_msg, session_id)?;
                    Err(eyre::eyre!(error_msg))
                }
            }
            Err(e) => {
                let user_id = &(0, address!("0x0000000000000000000000000000000000000000"));
                let error_msg = self.process_error(user_id, e.to_string(), session_id)?;
                return Err(eyre::eyre!(error_msg));
            }
        }
    }

    fn process_outgoing(&self, response: Q) -> Result<HashSet<(SessionId, String)>> {
        let mut result: HashSet<(SessionId, String)>;
        result = HashSet::new();

        let cloned_response = response.clone();
        let user_id = cloned_response.get_user_id();
        
        //let mut response = response;        
        if let Ok(sessions) = self.user_plugin.get_user_sessions(&user_id) {
            for session in sessions {
                let mut response = response.clone();   
                response.set_seq_num(self.seq_num_plugin.next_seq_num(&session));
         
                if let Ok(message) = self.serde_plugin.process_outgoing(response) {
                    result.insert((session, message));
                } else{
                    return Err(eyre::eyre!("Cannot serialize response."));
                }
            }
        } else {
            let session_id = cloned_response.get_session_id();
            let mut response = response.clone();   
            response.set_seq_num(self.seq_num_plugin.next_seq_num(&session_id.clone()));
         
            if let Ok(message) = self.serde_plugin.process_outgoing(response) {
                result.insert((session_id.clone(), message));
            } else{
                return Err(eyre::eyre!("Cannot serialize response."));
            }
        }

        return Ok(result);        
    }
    

    fn create_session(&self, session_id: &SessionId) -> Result<()> {
        self.seq_num_plugin.create_session(session_id)
    }

    fn destroy_session(&self, session_id: &SessionId) -> Result<()> {
        self.seq_num_plugin.destroy_session(session_id)
    }
    
}