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
use crate::trading_session::TradingSessionBuilder;
use crate::error_classifier::ErrorClassifier;
use crate::session_termination::{SessionTerminationResult};

pub struct Session {
    command_tx: UnboundedSender<Command>,
    session_loop: AsyncLoop<SessionTerminationResult>,
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

    pub async fn stop(&mut self) -> Result<SessionTerminationResult, Either<JoinError, Report>> {
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
            let on_error = |reason| {
                tracing::warn!("{:?}", reason);
                observer
                    .read()
                    .publish_single(OrderConnectorNotification::SessionLogout {
                        session_id: session_id.clone(),
                        reason,
                        timestamp: Utc::now(),
                    })
            };

            let mut trading_session = match TradingSessionBuilder::build(&credentials).await {
                Err(err) => {
                    let error_msg = format!("Failed to login: {:?}", err);
                    on_error(error_msg.clone());
                    
                    if ErrorClassifier::is_disconnection_error(&err) {
                        return SessionTerminationResult::disconnection(
                            Some(credentials),
                            session_id,
                        );
                    } else {
                        return SessionTerminationResult::network_error(
                            Some(credentials),
                            session_id,
                            error_msg,
                        );
                    }
                }
                Ok(s) => s,
            };

            if let Err(err) = trading_session.logon().await {
                let error_msg = format!("Failed to login: {:?}", err);
                on_error(error_msg.clone());

                if ErrorClassifier::is_disconnection_error(&err) {
                    return SessionTerminationResult::disconnection(
                        Some(credentials),
                        session_id,
                    );
                } else {
                    return SessionTerminationResult::network_error(
                        Some(credentials),
                        session_id,
                        error_msg,
                    );
                }
            }

            if let Err(err) = trading_session.get_exchange_info(symbols).await {
                let error_msg = format!("Failed to get exchange info: {:?}", err);
                on_error(error_msg.clone());
                
                if ErrorClassifier::is_disconnection_error(&err) {
                    return SessionTerminationResult::disconnection(
                        Some(credentials),
                        session_id,
                    );
                } else {
                    return SessionTerminationResult::network_error(
                        Some(credentials),
                        session_id,
                        error_msg,
                    );
                }
            }

            let user_data = match trading_session.get_user_data().await {
                Err(err) => {
                    let error_msg = format!("Failed to obtain user-data: {:?}", err);
                    on_error(error_msg.clone());
                    
                    if ErrorClassifier::is_disconnection_error(&err) {
                        return SessionTerminationResult::disconnection(
                            Some(credentials),
                            session_id,
                        );
                    } else {
                        return SessionTerminationResult::network_error(
                            Some(credentials),
                            session_id,
                            error_msg,
                        );
                    }
                }
                Ok(s) => s,
            };

            observer
                .read()
                .publish_single(OrderConnectorNotification::SessionLogon {
                    session_id: session_id.clone(),
                    timestamp: Utc::now(),
                });

            user_data.subscribe(observer.clone());

            loop {
                select! {
                    _ = cancel_token.cancelled() => {
                        break;
                    },
                    Some(command) = command_rx.recv() => {
                        if let Err(err) = trading_session.send_command(command, &observer).await {
                            tracing::warn!("Failed to send command: {:?}", err);
                            
                            if ErrorClassifier::is_disconnection_error(&err) {
                                user_data.unsubscribe().await;
                                
                                return SessionTerminationResult::disconnection(
                                    Some(credentials),
                                    session_id,
                                );
                            }
                        }
                    },
                }
            }

            user_data.unsubscribe().await;

            observer
                .read()
                .publish_single(OrderConnectorNotification::SessionLogout {
                    session_id: session_id_clone,
                    reason: "Session ended".to_owned(),
                    timestamp: Utc::now(),
                });

            tracing::info!("Session loop exited gracefully");
            SessionTerminationResult::graceful(credentials, session_id)
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
