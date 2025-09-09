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
    order_sender::order_connector::{OrderConnectorNotification, SessionId},
};
use tokio::{
    select,
    sync::mpsc::{UnboundedReceiver, UnboundedSender},
};
use tokio::{task::JoinError, time::sleep};
use tokio_util::sync::CancellationToken;

use crate::credentials::Credentials;
use crate::session_completion::SessionCompletionResult;
use crate::session_error::SessionError;
use crate::trading_session::{TradingSession, TradingSessionBuilder, TradingUserData};
use crate::{binance_order_sending::BinanceFeeCalculator, command::Command};

struct SessionState {
    trading_session: Option<TradingSession>,
    user_data: Option<TradingUserData>,
    credentials: Credentials,
    session_id: SessionId,
    observer: Arc<AtomicLock<SingleObserver<OrderConnectorNotification>>>,
    symbols: Vec<Symbol>,
    fee_calculator: BinanceFeeCalculator,
}

impl SessionState {
    fn new(
        credentials: Credentials,
        observer: Arc<AtomicLock<SingleObserver<OrderConnectorNotification>>>,
        symbols: Vec<Symbol>,
        fee_calculator: BinanceFeeCalculator,
    ) -> Self {
        let session_id = credentials.into_session_id();
        Self {
            trading_session: None,
            user_data: None,
            credentials,
            session_id,
            observer,
            symbols,
            fee_calculator,
        }
    }

    async fn build_trading_session(&mut self) -> Result<(), SessionError> {
        match TradingSessionBuilder::build(&self.credentials).await {
            Ok(ts) => {
                tracing::debug!("TradingSession built successfully");
                self.trading_session = Some(ts);
                Ok(())
            }
            Err(err) => {
                self.log_error("session build", &err);
                Err(err)
            }
        }
    }

    async fn perform_logon(&mut self) -> Result<(), SessionError> {
        if let Some(ref mut ts) = self.trading_session {
            ts.logon().await?;
            tracing::debug!("Logon completed successfully");
        }
        Ok(())
    }

    async fn get_exchange_info(&mut self) -> Result<(), SessionError> {
        if let Some(ref mut ts) = self.trading_session {
            ts.get_exchange_info(self.symbols.clone()).await?;
            tracing::debug!("Exchange info obtained successfully");
        }
        Ok(())
    }

    async fn get_user_data(&mut self) -> Result<(), SessionError> {
        if let Some(ref mut ts) = self.trading_session {
            let ud = ts.get_user_data().await?;
            tracing::debug!("User data obtained successfully");
            self.user_data = Some(ud);

            self.observer
                .read()
                .publish_single(OrderConnectorNotification::SessionLogon {
                    session_id: self.session_id.clone(),
                    timestamp: Utc::now(),
                });
        }
        Ok(())
    }

    fn log_error(&self, phase: &str, error: &SessionError) {
        tracing::warn!(
            "Session {} failed during {}: {}",
            self.session_id,
            phase,
            error
        );
        self.observer
            .read()
            .publish_single(OrderConnectorNotification::SessionLogout {
                session_id: self.session_id.clone(),
                reason: format!("Failed during {}: {}", phase, error),
                timestamp: Utc::now(),
            });
    }

    async fn run_main_loop(
        &mut self,
        mut command_rx: UnboundedReceiver<Command>,
        cancel_token: CancellationToken,
    ) -> Result<UnboundedReceiver<Command>, (SessionError, UnboundedReceiver<Command>)> {
        // TODO: Configure me, move me to SessiontSetupState
        let ping_period = std::time::Duration::from_secs(3);

        if let (Some(ref mut ts), Some(ref ud)) = (&mut self.trading_session, &self.user_data) {
            ud.subscribe(self.fee_calculator.clone(), self.observer.clone());

            tracing::info!("Session {} entering main loop", self.session_id);

            loop {
                select! {
                    _ = cancel_token.cancelled() => {
                        tracing::info!("Session {} cancelled", self.session_id);
                        break;
                    }
                    _ = sleep(ping_period) => {
                        if let Err(err) = ts.ping_connection().await {
                            tracing::warn!("Session connection lost");
                            return Err((err, command_rx));
                        }
                    }
                    Some(command) = command_rx.recv() => {
                        if let Err(err) = ts.send_command(command, &self.observer).await {
                            tracing::warn!("Command failed in session {}: {}", self.session_id, err);
                        }
                    }
                }
            }

            tracing::debug!("Unsubscribing from user data stream");
            ud.unsubscribe().await;
        }

        // Command receiver must be returned, and passed over to reconnection
        // or alternatively all commands need to be replied with cancelled!
        Ok(command_rx)
    }
}

pub struct Session {
    command_tx: UnboundedSender<Command>,
    session_loop: AsyncLoop<(UnboundedReceiver<Command>, SessionCompletionResult)>,
}

impl Session {
    pub fn new(command_tx: UnboundedSender<Command>) -> Self {
        Self {
            command_tx,
            session_loop: AsyncLoop::new(),
        }
    }

    pub fn send_command(&self, command: Command) -> Result<(), Command> {
        self.command_tx.send(command).map_err(|e| e.0)
    }

    pub async fn stop(
        &mut self,
    ) -> Result<(UnboundedReceiver<Command>, SessionCompletionResult), Either<JoinError, Report>>
    {
        self.session_loop.stop().await
    }

    pub fn has_stopped(&self) -> bool {
        self.session_loop.has_stopped()
    }

    pub fn start(
        &mut self,
        command_rx: UnboundedReceiver<Command>,
        observer: Arc<AtomicLock<SingleObserver<OrderConnectorNotification>>>,
        credentials: Credentials,
        symbols: Vec<Symbol>,
        fee_calculator: BinanceFeeCalculator,
    ) -> Result<()> {
        self.session_loop.start(async move |cancel_token| {
            tracing::info!("Session loop started");
            let session_id_clone = credentials.into_session_id();

            let mut session_state =
                SessionState::new(credentials, observer, symbols, fee_calculator);

            // Attempt to setup the session - early return on first error
            let setup_result = async {
                session_state.build_trading_session().await?;
                session_state.perform_logon().await?;
                session_state.get_exchange_info().await?;
                session_state.get_user_data().await?;
                Ok::<(), SessionError>(())
            }
            .await;

            match setup_result {
                Ok(()) => {
                    // Setup succeeded, run main loop
                    match session_state.run_main_loop(command_rx, cancel_token).await {
                        Err((error, command_rx)) => {
                            let should_return_credentials = error.should_reconnect();
                            tracing::warn!(
                                "Session {} completed with error: {} (reconnect: {})",
                                session_state.session_id,
                                error,
                                should_return_credentials
                            );

                            (
                                command_rx,
                                SessionCompletionResult::error(
                                    error,
                                    if should_return_credentials {
                                        Some(session_state.credentials)
                                    } else {
                                        None
                                    },
                                    session_state.session_id,
                                ),
                            )
                        }
                        Ok(command_rx) => {
                            tracing::info!(
                                "Session {} completed successfully",
                                session_state.session_id
                            );
                            session_state.observer.read().publish_single(
                                OrderConnectorNotification::SessionLogout {
                                    session_id: session_id_clone,
                                    reason: String::from("Session ended gracefully"),
                                    timestamp: Utc::now(),
                                },
                            );
                            (
                                command_rx,
                                SessionCompletionResult::success(session_state.credentials),
                            )
                        }
                    }
                }
                Err(error) => {
                    // Setup failed
                    let should_return_credentials = error.should_reconnect();
                    tracing::warn!(
                        "Session {} setup failed: {} (reconnect: {})",
                        session_id_clone,
                        error,
                        should_return_credentials
                    );

                    (
                        command_rx,
                        SessionCompletionResult::error(
                            error,
                            if should_return_credentials {
                                Some(session_state.credentials)
                            } else {
                                None
                            },
                            session_id_clone,
                        ),
                    )
                }
            }
        });

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::credentials::Credentials;
    use parking_lot::RwLock as AtomicLock;
    use std::{collections::HashMap, sync::Arc};
    use symm_core::{
        core::{functional::SingleObserver, test_util::get_mock_asset_name_1},
        market_data::exchange_rates::FixedExchangeRates,
    };
    use tokio::sync::mpsc::unbounded_channel;

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
        assert!(matches!(
            received_command.unwrap(),
            Command::EnableTrading(true)
        ));
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
        assert!(matches!(result, Err(_)));
    }

    #[test]
    fn test_session_setup_state_creation() {
        let credentials = create_mock_credentials();
        let observer = Arc::new(AtomicLock::new(SingleObserver::new()));
        let symbols = vec![get_mock_asset_name_1()];
        let exchange_rates = Arc::new(FixedExchangeRates::new(HashMap::new()));
        let fee_calculator = BinanceFeeCalculator::new(exchange_rates, get_mock_asset_name_1());

        let setup_state = SessionState::new(credentials, observer, symbols.clone(), fee_calculator);

        // Verify initial state
        assert!(setup_state.trading_session.is_none());
        assert!(setup_state.user_data.is_none());
        assert_eq!(setup_state.session_id, "test_account".into());
        assert_eq!(setup_state.symbols, symbols);
    }

    #[tokio::test]
    async fn test_session_setup_rate_limit_error() {
        use crate::session_error::SessionError;

        // Test rate limit error (should reconnect)
        let error = SessionError::RateLimitExceeded {
            message: "Rate limit exceeded".to_string(),
        };

        // Verify error properties
        assert!(error.should_reconnect());
        assert!(error.to_string().contains("Rate limit exceeded"));
    }
}
