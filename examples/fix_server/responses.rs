use axum_fix_server::messages::{FixMessage, FixMessageBuilder, SessionId, ServerResponse as AxumServerResponse};
use eyre::{eyre, Result};
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Debug)]
pub struct ExampleResponse {
    #[serde(skip)]
    pub session_id: SessionId,
    pub ack: bool,
    pub side: String,
}

#[derive(Serialize, Deserialize, Debug)]
pub enum Response {
    Ack {
        #[serde(skip)]
        session_id: SessionId,
        ack: bool,
        side: String,
    },
    Nak {
        #[serde(skip)]
        session_id: SessionId,
        ack: bool,
        side: String,
        reason: String,
    },
}

impl AxumServerResponse for Response {
    fn get_session_id(&self) -> &SessionId {
        match self {
            Response::Ack { session_id, .. } => session_id,
            Response::Nak { session_id, .. } => session_id,
        }
    }

    fn serialize_into_fix(&self, _builder: FixMessageBuilder) -> Result<FixMessage, eyre::Error> {
        // Serialize the response to JSON
        let json_str = serde_json::to_string(self)
            .map_err(|e| eyre!("Failed to serialize Response: {}", e))?;
        // Construct a FixMessage with the serialized data in the body
        println!("serialize_into_fix: {}", json_str);
        Ok(FixMessage(json_str.to_owned()))
    }
}

impl AxumServerResponse for ExampleResponse {
    fn get_session_id(&self) -> &SessionId {
        &self.session_id
    }

    fn serialize_into_fix(&self, _builder: FixMessageBuilder) -> Result<FixMessage, eyre::Error> {
        // Serialize the response to JSON
        let json_str = serde_json::to_string(self)
            .map_err(|e| eyre!("Failed to serialize ExampleResponse: {}", e))?;
        // Construct a FixMessage with the serialized data in the body
        println!("serialize_into_fix: {}", json_str);
        Ok(FixMessage(json_str.to_owned()))
    }
}