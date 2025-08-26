use chrono::{DateTime, Utc};
use symm_core::order_sender::order_connector::SessionId;
use crate::credentials::Credentials;
use crate::session_error::SessionError;

#[derive(Debug)]
pub enum SessionCompletionResult {
    Success(Credentials),
    Error { 
        error: SessionError, 
        credentials: Option<Credentials>,
        session_id: SessionId,
    },
}

impl SessionCompletionResult {
    pub fn success(credentials: Credentials) -> Self {
        Self::Success(credentials)
    }
    
    pub fn error(error: SessionError, credentials: Option<Credentials>, session_id: SessionId) -> Self {
        Self::Error { error, credentials, session_id }
    }
    
    pub fn should_reconnect(&self) -> bool {
        match self {
            Self::Success(_) => false,
            Self::Error { error, .. } => error.should_reconnect(),
        }
    }
    
    pub fn get_credentials(self) -> Option<Credentials> {
        match self {
            Self::Success(credentials) => Some(credentials),
            Self::Error { credentials, .. } => credentials,
        }
    }

    pub fn get_error(&self) -> Option<&SessionError> {
        match self {
            Self::Success(_) => None,
            Self::Error { error, .. } => Some(error),
        }
    }

    pub fn get_session_id(&self) -> Option<&SessionId> {
        match self {
            Self::Success(_) => None,
            Self::Error { session_id, .. } => Some(session_id),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session_error::SessionError;

    fn create_mock_credentials() -> Credentials {
        Credentials::new(
            String::from("test_account"),
            true,
            || Some(String::from("test_key")),
            || Some(String::from("test_secret")),
            || None,
            || None,
        )
    }

    #[test]
    fn test_success_completion() {
        let credentials = create_mock_credentials();
        let result = SessionCompletionResult::success(credentials.clone());

        assert!(!result.should_reconnect());
        assert!(result.get_error().is_none());
        assert_eq!(result.get_credentials().unwrap().account_name(), credentials.account_name());
    }

    #[test]
    fn test_error_completion_with_reconnect() {
        let credentials = create_mock_credentials();
        let session_id = credentials.into_session_id();
        let error = SessionError::Disconnection {
            message: String::from("Connection lost")
        };

        let result = SessionCompletionResult::error(error, Some(credentials), session_id);

        assert!(result.should_reconnect());
        assert!(result.get_error().is_some());
        assert!(result.get_credentials().is_some());
    }

    #[test]
    fn test_error_completion_without_reconnect() {
        let credentials = create_mock_credentials();
        let session_id = credentials.into_session_id();
        let error = SessionError::AuthenticationError {
            message: String::from("Invalid API key")
        };

        let result = SessionCompletionResult::error(error, None, session_id);

        assert!(!result.should_reconnect());
        assert!(result.get_error().is_some());
        assert!(result.get_credentials().is_none());
    }
}