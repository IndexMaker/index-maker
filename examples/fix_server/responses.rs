use alloy::{consensus::Header};
use axum_fix_server::{
    messages::{FixMessage, FixMessageBuilder, ServerResponse as AxumServerResponse, SessionId},
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

// #[derive(Serialize, Deserialize, Debug)]
// #[serde(untagged)]
// pub enum OldResponse {
//     ACK {
//         #[serde(skip)]
//         session_id: SessionId,
//         StandardHeader: FixHeader,
//         #[serde(flatten)]
//         Body: ACKBody,
//         StandardTrailer: FixTrailer,
//     },
//     NAK {
//         #[serde(skip)]
//         session_id: SessionId,
//         StandardHeader:Header,
//         #[serde(flatten)]
//         Body: NAKBody,
//         StandardTrailer: String,
//     },
//     ExecutionReport {
//         #[serde(skip)]
//         session_id: SessionId,
//         StandardHeader:Header,
//         #[serde(flatten)]
//         Body: ExecReportBody,
//         StandardTrailer: String,
//     }
// }


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

    fn serialize_into_fix(&self, builder: FixMessageBuilder) -> Result<FixMessage, eyre::Error> {
        // Serialize the response to JSON
        let json_str = serde_json::to_string(self)
            .map_err(|e| eyre!("Failed to serialize ExampleResponse: {}", e))?;
        // Construct a FixMessage with the serialized data in the body
        println!("serialize_into_fix: {}",json_str);
        Ok(FixMessage (json_str.to_owned()))
        // Ok(FixMessage(
        //     "this is a response, not a good one, but it's something".to_owned(),
        // ))
    }
}

impl AxumServerResponse for ExampleResponse {
    fn get_session_id(&self) -> &SessionId {
        &self.session_id
    }

    fn serialize_into_fix(&self, builder: FixMessageBuilder) -> Result<FixMessage, eyre::Error> {
        // Serialize the response to JSON
        let json_str = serde_json::to_string(self)
            .map_err(|e| eyre!("Failed to serialize ExampleResponse: {}", e))?;
        // Construct a FixMessage with the serialized data in the body
        println!("serialize_into_fix: {}",json_str);
        Ok(FixMessage (json_str.to_owned()))
        // Ok(FixMessage(
        //     "this is a response, not a good one, but it's something".to_owned(),
        // ))
    }
}