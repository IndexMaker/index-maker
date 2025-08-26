use std::sync::Arc;

use chrono::Utc;
use eyre::{eyre, Report, Result};
use itertools::Either;
use parking_lot::RwLock as AtomicLock;
use symm_core::{
    core::{
        async_loop::AsyncLoop,
        bits::Symbol,
        functional::{PublishSingle, SingleObserver},
    },
    order_sender::order_connector::OrderConnectorNotification,
};
use tokio::task::JoinError;
use tokio::{
    select,
    sync::mpsc::{UnboundedReceiver, UnboundedSender},
};

use crate::command::Command;
use crate::credentials::Credentials;
use crate::trading_session::{TradingSession, TradingSessionBuilder, TradingUserData};
use crate::session_completion::SessionCompletionResult;
use crate::session_error::SessionError;

pub struct Session {
    command_tx: UnboundedSender<Command>,
    session_loop: AsyncLoop<SessionCompletionResult>,
}

impl Session {
    pub fn new(command_tx: UnboundedSender<Command>) -> Self {
        Self {
            command_tx,
            session_loop: AsyncLoop::new(),
        }
    }

    pub fn send_command(&self, command: Command) -> Result<()> {
        self.command_tx
            .send(command)
            .map_err(|err| eyre!("Failed to send command to session {}", err))
    }

    pub async fn stop(&mut self) -> Result<SessionCompletionResult, Either<JoinError, Report>> {
        self.session_loop.stop().await
    }

    pub fn start(
        &mut self,
        mut command_rx: UnboundedReceiver<Command>,
        observer: Arc<AtomicLock<SingleObserver<OrderConnectorNotification>>>,
        credentials: Credentials,
        symbols: Vec<Symbol>,
    ) -> Result<()> {
        self.session_loop.start(async move |cancel_token| {
            tracing::info!("Session loop started");
            let session_id = credentials.into_session_id();
            let session_id_clone = session_id.clone();

            // Single mutable variable to track session state (reviewer requirement)
            let mut session_error: Option<SessionError> = None;
            let mut trading_session: Option<TradingSession> = None;
            let mut user_data: Option<TradingUserData> = None;

            // Helper function for consistent error logging
            let log_error = |phase: &str, error: &SessionError| {
                tracing::warn!("Session {} failed during {}: {}", session_id, phase, error);
                observer.read().publish_single(OrderConnectorNotification::SessionLogout {
                    session_id: session_id.clone(),
                    reason: format!("Failed during {}: {}", phase, error),
                    timestamp: Utc::now(),
                });
            };

            // SETUP PHASE 1: Build TradingSession
            if session_error.is_none() {
                match TradingSessionBuilder::build(&credentials).await {
                    Ok(ts) => {
                        tracing::debug!("TradingSession built successfully");
                        trading_session = Some(ts);
                    }
                    Err(err) => {
                        log_error("session build", &err);
                        session_error = Some(err);
                    }
                }
            }

            // SETUP PHASE 2: Logon
            if session_error.is_none() {
                if let Some(ref mut ts) = trading_session {
                    if let Err(err) = ts.logon().await {
                        log_error("logon", &err);
                        session_error = Some(err);
                    } else {
                        tracing::debug!("Logon completed successfully");
                    }
                }
            }

            // SETUP PHASE 3: Get Exchange Info
            if session_error.is_none() {
                if let Some(ref mut ts) = trading_session {
                    if let Err(err) = ts.get_exchange_info(symbols.clone()).await {
                        log_error("exchange info", &err);
                        session_error = Some(err);
                    } else {
                        tracing::debug!("Exchange info obtained successfully");
                    }
                }
            }

            // SETUP PHASE 4: Get User Data
            if session_error.is_none() {
                if let Some(ref mut ts) = trading_session {
                    match ts.get_user_data().await {
                        Ok(ud) => {
                            tracing::debug!("User data obtained successfully");
                            user_data = Some(ud);

                            // Publish successful logon only after complete setup
                            observer.read().publish_single(OrderConnectorNotification::SessionLogon {
                                session_id: session_id.clone(),
                                timestamp: Utc::now(),
                            });
                        }
                        Err(err) => {
                            log_error("user data", &err);
                            session_error = Some(err);
                        }
                    }
                }
            }

            // MAIN LOOP: Only run if setup succeeded
            if session_error.is_none() {
                if let (Some(ref mut ts), Some(ref ud)) = (&mut trading_session, &user_data) {
                    ud.subscribe(observer.clone());
                    tracing::info!("Session {} entering main loop", session_id);

                    loop {
                        select! {
                            _ = cancel_token.cancelled() => {
                                tracing::info!("Session {} cancelled", session_id);
                                break;
                            }
                            Some(command) = command_rx.recv() => {
                                if let Err(err) = ts.send_command(command, &observer).await {
                                    tracing::warn!("Command failed in session {}: {}", session_id, err);

                                    // Only break on errors that should trigger reconnection
                                    if err.should_reconnect() {
                                        session_error = Some(err);
                                        break;
                                    }
                                    // For non-reconnection errors, continue processing
                                }
                            }
                        }
                    }

                    // Cleanup after main loop
                    tracing::debug!("Unsubscribing from user data stream");
                    ud.unsubscribe().await;
                }
            }

            // SINGLE EXIT POINT: Create SessionCompletionResult from session_error
            match session_error {
                Some(error) => {
                    let should_return_credentials = error.should_reconnect();
                    tracing::warn!(
                        "Session {} completed with error: {} (reconnect: {})",
                        session_id, error, should_return_credentials
                    );

                    SessionCompletionResult::error(
                        error,
                        if should_return_credentials { Some(credentials) } else { None },
                        session_id,
                    )
                }
                None => {
                    tracing::info!("Session {} completed successfully", session_id);
                    observer.read().publish_single(OrderConnectorNotification::SessionLogout {
                        session_id: session_id_clone,
                        reason: String::from("Session ended gracefully"),
                        timestamp: Utc::now(),
                    });
                    SessionCompletionResult::success(credentials)
                }
            }
        });

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use parking_lot::RwLock as AtomicLock;
    use symm_core::core::{
        functional::SingleObserver,
        test_util::{get_mock_asset_name_1, get_mock_atomic_bool_pair, test_mock_atomic_bool}
    };
    use tokio::sync::mpsc::unbounded_channel;
    use crate::credentials::Credentials;

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

    #[tokio::test]
    async fn test_session_creation() {
        let (command_tx, _command_rx) = unbounded_channel();
        let session = Session::new(command_tx);
        
        // Verify session is created with correct initial state
        assert!(!session.session_loop.has_stopped());
    }

    #[tokio::test]
    async fn test_send_command_success() {
        let (command_tx, mut command_rx) = unbounded_channel();
        let session = Session::new(command_tx);
        
        let test_command = Command::EnableTrading(true);
        let result = session.send_command(test_command);
        
        assert!(result.is_ok());
        
        // Verify command was sent
        let received_command = command_rx.recv().await;
        assert!(received_command.is_some());
        assert!(matches!(received_command.unwrap(), Command::EnableTrading(true)));
    }

    #[tokio::test]
    async fn test_send_command_channel_closed() {
        let (command_tx, command_rx) = unbounded_channel();
        let session = Session::new(command_tx);
        
        // Close the receiver
        drop(command_rx);
        
        let test_command = Command::EnableTrading(true);
        let result = session.send_command(test_command);
        
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Failed to send command"));
    }
}
