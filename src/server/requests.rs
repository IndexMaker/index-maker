use axum_fix_server::{
    messages::{FixMessage, ServerRequest as AxumServerRequest, SessionId},
    plugins::{seq_num_plugin::WithSeqNumPlugin, user_plugin::WithUserPlugin},
};
use eyre::{eyre, Result};
use serde::{Deserialize, Serialize};
use symm_core::core::bits::Address;

use crate::server::fix_messages::*;


#[derive(Serialize, Deserialize, Debug)]
pub struct FixRequest {
    #[serde(skip)]
    pub session_id: SessionId,
    pub standard_header: FixHeader,
    pub chain_id: u32,
    pub address: Address,
    #[serde(flatten)]
    pub body: Body,
    pub standard_trailer: FixTrailer,
}

impl WithSeqNumPlugin for FixRequest {
    fn get_seq_num(&self) -> u32 {
        self.standard_header.SeqNum
    }

    fn set_seq_num(&mut self, seq_num: u32) {
        self.standard_header.SeqNum = seq_num;
    }
}

impl WithUserPlugin for FixRequest {
    fn get_user_id(&self) -> (u32, Address) {
        (self.chain_id, self.address)
    }
}

impl AxumServerRequest for FixRequest {
    fn deserialize_from_fix(
        message: FixMessage,
        this_session_id: &SessionId,
    ) -> Result<Self, eyre::Error> {
        println!("{}: {}", this_session_id, message);

        let mut request: FixRequest = serde_json::from_str(&message.to_string())
            .map_err(|e| eyre!("Failed to deserialize FixMessage into FixRequest: {}", e))?;

        // Set the session_id
        request.session_id = this_session_id.clone();
        println!("deserialize_from_fix: Session ID set to {}", this_session_id);
        Ok(request)
    }
}
