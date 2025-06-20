use alloy::transports::http::reqwest::header;
use axum_fix_server::messages::{FixMessage, SessionId, ServerRequest as AxumServerRequest};
use eyre::{eyre, Result};
use index_maker::core::bits::Address;
use serde::{Deserialize, Serialize};

use crate::fix_messages::*;

// #[derive(Serialize, Deserialize, Debug)]
// pub struct ExampleRequest {
//     #[serde(skip)]
//     pub session_id: SessionId,
//     pub address: Address,
//     pub quantity: i32,
// }

#[derive(Serialize, Deserialize, Debug)]
#[serde(untagged)]
pub enum Request {
    #[serde(untagged)]
    NewOrderSingle{
        #[serde(skip)]
        session_id: SessionId,
        StandardHeader: FixHeader,
        #[serde(flatten)]
        Body: NewOrderBody,
        StandardTrailer: FixTrailer,
    }
}



impl AxumServerRequest for Request {
    // fn deserialize_from_fix(
    //     message: FixMessage,
    //     session_id: SessionId,
    // ) -> Result<Self, eyre::Error> {
    //     println!("{}: {}", session_id, message);
    //     let header: FixHeader = serde_json::from_str(&message.to_string())
    //         .map_err(|e| eyre!("Failed to deserialize FixMessage Header: {}", e))?;

    //     match header.MsgType.as_str() {
    //         "NewOrderSingle" => {
    //             let mut request: Request = serde_json::from_str(&message.to_string())
    //                 .map_err(|e| eyre!("Failed to deserialize FixMessage: {}", e))?;
    //             if let Request::NewOrderSingle { ref mut session_id, .. } = request {
    //                 *session_id = session_id.clone();
    //                 println!("deserialize_from_fix: Session ID set to {}", session_id);
    //             }
    //             Ok(request)
    //         },
    //         _ => Err(eyre!("Unsupported message type: {}", header.MsgType)),
    //     }
    // }

    fn deserialize_from_fix(
        message: FixMessage,
        session_id: SessionId,
    ) -> Result<Self, eyre::Error> {
        println!("{}: {}", session_id, message);
        // First, parse the message into a generic serde_json::Value to inspect fields
        let value: serde_json::Value = serde_json::from_str(&message.to_string())
            .map_err(|e| eyre!("Failed to parse FixMessage as JSON: {}", e))?;

        let msg_type = if let Some(header) = value.get("StandardHeader") {
            header.get("MsgType").and_then(|msg_type| msg_type.as_str())
        } else {
            return Err(eyre!("StandardHeader field not found or not an object in FixMessage"));
        };

        println!("msg_type: {:?}", msg_type);

        match msg_type {
            Some("NewOrderSingle") => {
                // Now deserialize the full Request since MsgType matches
                let mut request: Request = serde_json::from_str(&message.to_string())
                    .map_err(|e| eyre!("Failed to deserialize FixMessage into Request: {}", e))?;

                if let Request::NewOrderSingle { ref mut session_id, .. } = request {
                    *session_id = session_id.clone();
                    println!("deserialize_from_fix: Session ID set to {}", session_id);
                }
                Ok(request)
            },
            Some(other) => Err(eyre!("Unsupported message type: {}", other)),
            None => Err(eyre!("MsgType field not found or not a string in StandardHeader")),
        }
    }
}

// impl axum_fix_server::plugins::server_plugin::Signer for ExampleRequest {
//     fn get_address(&self) -> &Address {
//         &self.address
//     }
// }

// impl AxumServerRequest for ExampleRequest {
//     fn deserialize_from_fix(
//         message: FixMessage,
//         session_id: SessionId,
//     ) -> Result<Self, eyre::Error> {
//         println!("{}: {}", session_id, message);
//         let header: FixHeader = serde_json::from_str(&message.to_string())
//             .map_err(|e| eyre!("Failed to deserialize FixMessage: {}", e))?;

//         match header.MsgType.as_str() {
//             "NewOrderSingle" => {
//                 let mut request: ExampleRequest = serde_json::from_str(&message.to_string())
//                     .map_err(|e| eyre!("Failed to deserialize FixMessage: {}", e))?;
//                 request.session_id = session_id.clone();
//                 println!("deserialize_from_fix: {} {} {}", request.session_id, request.address, request.quantity);
//                 Ok(request)
//             },
//             _ => Err(eyre!("Unsupported message type: {}", header.MsgType)),
//         }
//     }
// }

// impl axum_fix_server::plugins::server_plugin::Signer for ExampleRequest {
//     fn get_address(&self) -> &Address {
//         &self.address
//     }
// }