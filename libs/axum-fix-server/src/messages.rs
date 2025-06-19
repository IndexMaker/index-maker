use std::fmt::Display;

use eyre::{eyre, Report, Result};

// // MyServerRequest
// ///
// /// Represents different types of requests that can be received by the server.
// pub enum MyServerRequest {
//     NewOrder { text: String, session_id: SessionId },
//     CancelOrder { session_id: SessionId },
// }

// /// ServerRequest
// ///
// /// A trait for deserializing server requests from messages. Implementors must define
// /// how to convert a FIX message into a specific request type.
// impl ServerRequest for MyServerRequest {
//     /// deserialize_from_fix
//     ///
//     /// Deserializes a FIX message into a specific server request type. Takes a `FixMessage` and a `SessionId`
//     /// as input and returns a `Result` containing the deserialized request or an error.
//     fn deserialize_from_fix(message: FixMessage, session_id: SessionId) -> Result<Self, Report> {
//         match message.0.as_str() {
//             s if s.starts_with("NewOrder") => Ok(MyServerRequest::NewOrder {
//                 text: message.0,
//                 session_id,
//             }),
//             s if s.starts_with("CancelOrder") => Ok(MyServerRequest::CancelOrder {
//                 session_id,
//             }),
//             _ => Err(eyre!("Oh no!")),
//         }
//     }
// }


// /// MyServerResponse
// ///
// /// Represents different types of responses the server can send to client requests.
// #[derive(Debug, Clone)]
// pub enum MyServerResponse {
//     NewOrderAck { text: String, session_id: SessionId },
//     NewOrderNak { session_id: SessionId },
//     CancelAck { session_id: SessionId },
//     CancelNak { session_id: SessionId },
// }

// /// ServerResponse
// ///
// /// A trait for server responses that defines methods for serialization into FIX messages
// /// and retrieving the associated session ID.
// impl ServerResponse for MyServerResponse {
//     /// serialize_into_fix
//     ///
//     /// Serializes the response into a FIX message using the provided `FixMessageBuilder`.
//     /// Returns a `Result` containing the serialized `FixMessage` or an error.
//     fn serialize_into_fix(&self, builder: FixMessageBuilder) -> Result<FixMessage, Report> {
//         match self {
//             MyServerResponse::NewOrderAck { text, .. } => {
//                 Ok(FixMessage("NewOrderAck: Acknowledged ".to_string() + text))
//             }
//             MyServerResponse::NewOrderNak { .. } => {
//                 Ok(FixMessage("NewOrderNak: Rejected".to_string()))
//             }
//             MyServerResponse::CancelAck { .. } => {
//                 Ok(FixMessage("CancelAck: Cancel Acknowledged".to_string()))
//             }
//             MyServerResponse::CancelNak { .. } => {
//                 Ok(FixMessage("CancelNak: Cancel Rejected".to_string()))
//             }
//         }
//     }

//     /// get_session_id
//     ///
//     /// Retrieves the `SessionId` associated with this response. Used to route the response
//     /// to the correct client session.
//     fn get_session_id(&self) -> &SessionId {
//         match self {
//             MyServerResponse::NewOrderAck { session_id, .. } => session_id,
//             MyServerResponse::NewOrderNak { session_id, .. } => session_id,
//             MyServerResponse::CancelAck { session_id, .. } => session_id,
//             MyServerResponse::CancelNak { session_id, .. } => session_id,
//         }
//     }
// }


/// FixMessage
///
/// Represents a FIX message as a simple string wrapper. Used for both incoming and
/// outgoing messages in the server communication.
pub struct FixMessage(pub String);

impl Display for FixMessage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<&str> for FixMessage {
    fn from(value: &str) -> Self {
        Self(value.into())
    }
}

/// FixMessageBuilder
///
/// A utility struct for building FIX messages.
pub struct FixMessageBuilder;

impl FixMessageBuilder {
    pub fn new() -> Self {
        Self
    }
}



#[derive(Default, Hash, Eq, PartialEq, Clone, Debug)]
pub struct SessionId(pub String);

impl Display for SessionId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "SessionId({})", self.0)
    }
}

impl From<&str> for SessionId {
    fn from(value: &str) -> Self {
        Self(value.into())
    }
}

pub trait ServerRequest
where
    Self: Sized,
{
    fn deserialize_from_fix(message: FixMessage, session_id: SessionId) -> Result<Self, Report>;
}

pub trait ServerResponse {
    fn get_session_id(&self) -> &SessionId;
    fn serialize_into_fix(&self, builder: FixMessageBuilder) -> Result<FixMessage, Report>;
}