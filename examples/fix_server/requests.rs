use axum_fix_server::messages::{FixMessage, SessionId, ServerRequest as AxumServerRequest};
use eyre::{eyre, Result};
use index_maker::core::bits::Address;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Debug)]
pub struct ExampleRequest {
    #[serde(skip)]
    pub session_id: SessionId,
    pub address: Address,
    pub quantity: i32,
}

impl AxumServerRequest for ExampleRequest {
    fn deserialize_from_fix(
        message: FixMessage,
        session_id: SessionId,
    ) -> Result<Self, eyre::Error> {
        println!("{}: {}", session_id, message);
        let mut request: ExampleRequest = serde_json::from_str(&message.to_string())
            .map_err(|e| eyre!("Failed to deserialize FixMessage: {}", e))?;
        request.session_id = session_id.clone();
        println!("deserialize_from_fix: {} {} {}", request.session_id, request.address, request.quantity);
        Ok(request)
    }
}

impl axum_fix_server::plugins::server_plugin::Signer for ExampleRequest {
    fn get_address(&self) -> &Address {
        &self.address
    }
}