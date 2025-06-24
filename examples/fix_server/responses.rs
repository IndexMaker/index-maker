use axum_fix_server::{
    messages::{FixMessage,  ServerResponse as AxumServerResponse, SessionId},
    plugins::seq_num_plugin::SeqNumPluginAux,
};
use eyre::{eyre, Result};
use serde::{Deserialize, Serialize};

use crate::fix_messages::*;

#[derive(Serialize, Deserialize, Debug)]
pub struct ExampleResponse {
    #[serde(skip)]
    pub session_id: SessionId,
    pub ack: bool,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct Response {
    #[serde(skip)]
    pub session_id: SessionId,
    pub standard_header: FixHeader,
    #[serde(flatten)]
    pub body: Body,
    pub standard_trailer: FixTrailer,
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
        println!("serialize_into_fix: {}",json_str);
        Ok(FixMessage (json_str.to_owned()))
    }
}

impl AxumServerResponse for ExampleResponse {
    fn get_session_id(&self) -> &SessionId {
        &self.session_id
    }

    fn serialize_into_fix(&self) -> Result<FixMessage, eyre::Error> {
        // Serialize the response to JSON
        let json_str = serde_json::to_string(self)
            .map_err(|e| eyre!("Failed to serialize ExampleResponse: {}", e))?;
        // Construct a FixMessage with the serialized data in the body
        println!("serialize_into_fix: {}",json_str);
        Ok(FixMessage (json_str.to_owned()))
    }
}