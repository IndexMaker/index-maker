use alloy::transports::http::reqwest::header;
use axum_fix_server::{
    messages::{FixMessage, ServerRequest as AxumServerRequest, SessionId},
    plugins::seq_num_plugin::SeqNumPluginAux,
};
use eyre::{eyre, Result};
use serde::{Deserialize, Serialize};
use index_maker::core::bits::Address;

use crate::fix_messages::*;

// #[derive(Serialize, Deserialize, Debug)]
// pub struct ExampleRequest {
//     #[serde(skip)]
//     pub session_id: SessionId,
//     pub address: Address,
//     pub quantity: i32,
// }

// #[derive(Serialize, Deserialize, Debug)]
// #[serde(untagged)]
// pub enum Request {
//     #[serde(untagged)]
//     NewOrderSingle{
//         #[serde(skip)]
//         session_id: SessionId,
//         StandardHeader: FixHeader,
//         #[serde(flatten)]
//         Body: NewOrderBody,
//         StandardTrailer: FixTrailer,
//     }
// }

#[derive(Serialize, Deserialize, Debug)]
pub struct Request {
    #[serde(skip)]
    pub session_id: SessionId,
    pub standard_header: FixHeader,
    #[serde(flatten)]
    pub body: Body,
    pub standard_trailer: FixTrailer,
}

impl Request {
    pub fn msg_type(&self) -> String {
        self.standard_header.MsgType.clone()
    }
}

impl SeqNumPluginAux for Request {
    fn get_seq_num(&self) -> u32 {
        self.standard_header.SeqNum
    }

    fn set_seq_num(&mut self, seq_num: u32) {
        self.standard_header.SeqNum = seq_num;
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
        this_session_id: SessionId,
    ) -> Result<Self, eyre::Error> {
        println!("{}: {}", this_session_id, message);

        let mut request: Request = serde_json::from_str(&message.to_string())
            .map_err(|e| eyre!("Failed to deserialize FixMessage into Request: {}", e))?;

        // Set the session_id
        request.session_id = this_session_id.clone();
        println!("deserialize_from_fix: Session ID set to {}", this_session_id);
        Ok(request)
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
