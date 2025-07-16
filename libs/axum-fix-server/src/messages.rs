use eyre::Result;
use serde::{Deserialize, Serialize};
use std::fmt::Display;
use symm_core::{core::bits::Address, string_id};

/// FixMessage
///
/// Represents a FIX message as a simple string wrapper. Used for both incoming and
/// outgoing messages in the server communication.
#[derive(Serialize, Deserialize, Debug)]
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

string_id!(SessionId);

pub trait ServerRequest
where
    Self: Sized,
{
    fn deserialize_from_fix(message: FixMessage, session_id: &SessionId) -> Result<Self>;
}

pub trait ServerResponse {
    fn get_session_id(&self) -> &SessionId;

    fn serialize_into_fix(&self) -> Result<FixMessage>;

    fn format_errors(
        user_id: &(u32, Address),
        session_id: &SessionId,
        error_msg: String,
        ref_seq_num: u32,
    ) -> Self;

    fn format_ack(
        user_id: &(u32, Address),
        session_id: &SessionId,
        ref_seq_num: u32,
    ) -> Self;
}
