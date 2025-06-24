use axum_fix_server::{
    messages::{FixMessage, ServerResponse as AxumServerResponse, SessionId},
    plugins::seq_num_plugin::SeqNumPluginAux,
};
use eyre::{eyre, Result};
use serde::{Deserialize, Serialize};

use crate::fix_messages::*;

#[derive(Serialize, Deserialize, Debug)]
pub struct Response {
    #[serde(skip)]
    pub session_id: SessionId,
    pub standard_header: FixHeader,
    #[serde(flatten)]
    pub body: Body,
    pub standard_trailer: FixTrailer,
}

impl Response {
    // New method to create a NAK response for errors
    pub fn create_nak(session_id: &SessionId, seq_num: u32, error_reason: String) -> Self {
        Response {
            session_id: session_id.clone(),
            standard_header: FixHeader::new("NAK".to_string()),
            body: Body::NAKBody {
                RefSeqNum: seq_num,
                Text: error_reason,
            },
            standard_trailer: FixTrailer::new(),
        }
    }
}

impl SeqNumPluginAux for Response {
    fn get_seq_num(&self) -> u32 {
        self.standard_header.SeqNum
    }

    fn set_seq_num(&mut self, seq_num: u32) {
        self.standard_header.SeqNum = seq_num;
    }
}

impl AxumServerResponse for Response {
    fn get_session_id(&self) -> &SessionId {
        &self.session_id
    }

    fn serialize_into_fix(&self) -> Result<FixMessage, eyre::Error> {
        // Serialize the response to JSON
        let json_str = serde_json::to_string(self)
            .map_err(|e| eyre!("Failed to serialize ExampleResponse: {}", e))?;
        // Construct a FixMessage with the serialized data in the body
        println!("serialize_into_fix: {}", json_str);
        Ok(FixMessage(json_str.to_owned()))
    }
    
    fn format_errors(session_id: &SessionId, error_msg: String, ref_seq_num: u32) -> Self {
        Response::create_nak(session_id, ref_seq_num, error_msg)
    }
    
}
