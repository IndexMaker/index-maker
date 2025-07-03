use axum_fix_server::{
    messages::{FixMessage, ServerResponse as AxumServerResponse, SessionId},
    plugins::{seq_num_plugin::WithSeqNumPlugin, user_plugin::WithUserPlugin},
};
use symm_core::core::bits::Address;
use eyre::{eyre, Result};
use serde::{Deserialize, Serialize};

use crate::server::fix::messages::*;


#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct FixResponse {
    #[serde(skip)]
    pub session_id: SessionId,
    pub standard_header: FixHeader,
    pub chain_id: u32,
    pub address: Address,
    #[serde(flatten)]
    pub body: Body,
    pub standard_trailer: FixTrailer,
}

impl FixResponse {
    // New method to create a NAK response for errors
    pub fn create_nak(user_id: &(u32, Address), session_id: &SessionId, seq_num: u32, error_reason: String) -> Self {
        FixResponse {
            session_id: session_id.clone(),
            standard_header: FixHeader::new("NAK".to_string()),
            chain_id: user_id.0,
            address: user_id.1,
            body: Body::NAKBody {
                ref_seq_num: seq_num,
                reason: error_reason,
            },
            standard_trailer: FixTrailer::new(),
        }
    }
}

impl WithSeqNumPlugin for FixResponse {
    fn get_seq_num(&self) -> u32 {
        self.standard_header.seq_num
    }

    fn set_seq_num(&mut self, seq_num: u32) {
        self.standard_header.seq_num = seq_num;
    }
}

impl WithUserPlugin for FixResponse {
    fn get_user_id(&self) -> (u32, Address) {
        (self.chain_id, self.address)
    }
}

impl AxumServerResponse for FixResponse {
    fn get_session_id(&self) -> &SessionId {
        &self.session_id
    }

    fn serialize_into_fix(&self) -> Result<FixMessage> {
        // Serialize the response to JSON
        let json_str = serde_json::to_string(self)
            .map_err(|e| eyre!("Failed to serialize message: {}", e))?;
        println!("serialize_into_fix: {}", json_str);
        Ok(FixMessage(json_str.to_owned()))
    }
    
    fn format_errors(user_id: &(u32, Address), session_id: &SessionId, error_msg: String, ref_seq_num: u32) -> Self {
        FixResponse::create_nak(user_id, session_id, ref_seq_num, error_msg)
    }
}
