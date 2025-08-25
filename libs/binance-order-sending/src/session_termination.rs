use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use symm_core::order_sender::order_connector::SessionId;
use crate::credentials::Credentials;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SessionTerminationReason {
    /// Normal shutdown requested by application
    Graceful,
    /// WebSocket connection lost unexpectedly
    WebSocketDisconnection,
    /// Credentials expired or became invalid
    CredentialExpiry,
    /// Network connectivity issues
    NetworkError(String),
    /// Rate limit exceeded, session terminated
    RateLimitViolation,
    /// Binance server error
    ServerError(String),
}

#[derive(Debug)]
pub struct SessionTerminationResult {
    pub credentials: Option<Credentials>,
    pub reason: SessionTerminationReason,
    pub session_id: SessionId,
    pub timestamp: DateTime<Utc>,
    pub should_reconnect: bool,
}

impl SessionTerminationResult {
    pub fn new(
        credentials: Option<Credentials>,
        reason: SessionTerminationReason,
        session_id: SessionId,
    ) -> Self {
        let should_reconnect = match &reason {
            SessionTerminationReason::Graceful => false,
            SessionTerminationReason::WebSocketDisconnection => true,
            SessionTerminationReason::CredentialExpiry => false,
            SessionTerminationReason::NetworkError(_) => true,
            SessionTerminationReason::RateLimitViolation => true,
            SessionTerminationReason::ServerError(_) => true,
        };

        Self {
            credentials,
            reason,
            session_id,
            timestamp: Utc::now(),
            should_reconnect,
        }
    }

    pub fn graceful(credentials: Credentials, session_id: SessionId) -> Self {
        Self::new(
            Some(credentials),
            SessionTerminationReason::Graceful,
            session_id,
        )
    }

    pub fn disconnection(
        credentials: Option<Credentials>,
        session_id: SessionId,
    ) -> Self {
        Self::new(
            credentials,
            SessionTerminationReason::WebSocketDisconnection,
            session_id,
        )
    }

    pub fn network_error(
        credentials: Option<Credentials>,
        session_id: SessionId,
        error: String,
    ) -> Self {
        Self::new(
            credentials,
            SessionTerminationReason::NetworkError(error),
            session_id,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use symm_core::order_sender::order_connector::SessionId;


    fn create_mock_credentials() -> Credentials {
        Credentials::new(
            "test_account".to_string(),
            true,
            || Some("test_key".to_string()),
            || Some("test_secret".to_string()),
            || None,
            || None,
        )
    }

    #[test]
    fn test_graceful_termination() {
        let credentials = create_mock_credentials();
        let session_id = SessionId::from("test_session");
        
        let result = SessionTerminationResult::graceful(credentials, session_id.clone());
        
        assert!(result.credentials.is_some());
        assert_eq!(result.session_id, session_id);
        assert!(!result.should_reconnect);
        assert!(matches!(result.reason, SessionTerminationReason::Graceful));
    }

    #[test]
    fn test_disconnection_termination() {
        let credentials = create_mock_credentials();
        let session_id = SessionId::from("test_session");
        
        let result = SessionTerminationResult::disconnection(
            Some(credentials), 
            session_id.clone(), 
        );
        
        assert!(result.credentials.is_some());
        assert_eq!(result.session_id, session_id);
        assert!(result.should_reconnect);
    }

    #[test]
    fn test_network_error_termination() {
        let credentials = create_mock_credentials();
        let session_id = SessionId::from("test_session");
        let error_msg = "Network unreachable".to_string();
        
        let result = SessionTerminationResult::network_error(
            Some(credentials), 
            session_id.clone(), 
            error_msg.clone()
        );
        
        assert!(result.credentials.is_some());
        assert_eq!(result.session_id, session_id);
        assert!(result.should_reconnect);
        assert!(matches!(
            result.reason, 
            SessionTerminationReason::NetworkError(ref msg) if msg == &error_msg
        ));
    }

    #[test]
    fn test_should_reconnect_logic() {
        let session_id = SessionId::from("test_session");
        
        // Should reconnect cases
        let reconnect_cases = vec![
            SessionTerminationReason::WebSocketDisconnection,
            SessionTerminationReason::NetworkError("test".to_string()),
            SessionTerminationReason::RateLimitViolation,
            SessionTerminationReason::ServerError("test".to_string()),
        ];
        
        for reason in reconnect_cases {
            let result = SessionTerminationResult::new(None, reason, session_id.clone());
            assert!(result.should_reconnect, "Should reconnect for {:?}", result.reason);
        }
        
        // Should NOT reconnect cases
        let no_reconnect_cases = vec![
            SessionTerminationReason::Graceful,
            SessionTerminationReason::CredentialExpiry,
        ];
        
        for reason in no_reconnect_cases {
            let result = SessionTerminationResult::new(None, reason, session_id.clone());
            assert!(!result.should_reconnect, "Should NOT reconnect for {:?}", result.reason);
        }
    }
}