use std::fmt::Display;
use symm_core::core::bits::Address;
use eyre::{Report, Result};
use serde::{Serialize, Deserialize};

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

impl Serialize for SessionId {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: serde::Serializer {
        serializer.serialize_str(&format!("{:?}", self.0))
    }
}

impl<'de> Deserialize<'de> for SessionId {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de> {
        let s = String::deserialize(deserializer)?;
        Ok(SessionId(s))
    }
}

pub trait ServerRequest
where
    Self: Sized,
{
    fn deserialize_from_fix(message: FixMessage, session_id: &SessionId) -> Result<Self>;
}


pub trait ServerResponse {
    fn get_session_id(&self) -> &SessionId;

    fn serialize_into_fix(&self) -> Result<FixMessage>;

    fn format_errors(user_id: &(u32, Address), session_id: &SessionId, error_msg: String, ref_seq_num: u32) -> Self;
}