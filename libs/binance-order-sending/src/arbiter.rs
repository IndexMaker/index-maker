use std::sync::Arc;

use chrono::Utc;
use eyre::Report;
use itertools::Either;
use parking_lot::RwLock as AtomicLock;
use symm_core::{
    core::{async_loop::AsyncLoop, bits::Symbol, functional::SingleObserver},
    order_sender::order_connector::{OrderConnectorNotification, SessionId},
};
use tokio::{select, sync::mpsc::UnboundedReceiver, task::JoinError};

use crate::{
    binance_order_sending::BinanceFeeCalculator, credentials::Credentials,
    session_completion::SessionCompletionResult, sessions::Sessions, subaccounts::SubAccounts,
};

/// Arbiter manages open sessions
///
/// When started it receives session credentials, and then
/// opens connection and passes credentials for logon. After
/// that session is running, and arbiter is forwarding session
/// commands to target sessions.
pub struct Arbiter {
    arbiter_loop: AsyncLoop<UnboundedReceiver<Credentials>>,
}

impl Arbiter {
    pub fn new() -> Self {
        Self {
            arbiter_loop: AsyncLoop::new(),
        }
    }

    pub async fn stop(
        &mut self,
    ) -> Result<UnboundedReceiver<Credentials>, Either<JoinError, Report>> {
        self.arbiter_loop.stop().await
    }

    pub fn start(
        &mut self,
        subaccounts: Arc<AtomicLock<SubAccounts>>,
        mut subaccount_rx: UnboundedReceiver<Credentials>,
        symbols: Vec<Symbol>,
        fee_calculator: BinanceFeeCalculator,
        sessions: Arc<AtomicLock<Sessions>>,
        observer: Arc<AtomicLock<SingleObserver<OrderConnectorNotification>>>,
    ) {
        self.arbiter_loop.start(async move |cancel_token| {
            tracing::info!("Loop started");
            loop {
                select! {
                    _ = cancel_token.cancelled() => {
                        break
                    },
                    Some(credentials) = subaccount_rx.recv() => {
                        let account_name = credentials.get_account_name();
                        match sessions.write().add_session(
                            credentials, symbols.clone(), fee_calculator.clone(), observer.clone()) {
                            Ok(_) => {
                                let mut suba = subaccounts.write();
                                if let Err(err) = suba.add_subaccount_taken(account_name) {
                                    tracing::warn!("Error storing taken session {:?}", err);
                                }
                            }
                            Err(err) => {
                                tracing::warn!("Error while creating session {:?}", err);
                            }
                        }
                    }
                }
            }

            // Enhanced shutdown with disconnection handling
            let all_sessions = sessions.write().drain_all_sessions();
            let completion_results = match Sessions::stop_all(all_sessions).await {
                Ok(res) => res,
                Err(err) => {
                    tracing::warn!("Error stopping sessions {:?}", err);
                    Vec::new()
                }
            };

            for result in completion_results {
                Self::process_session_completion(
                    result,
                    &sessions,
                    &symbols,
                    fee_calculator.clone(),
                    &observer,
                ).await;
            }

            tracing::info!("Loop exited");
            subaccount_rx
        });
    }

    async fn process_session_completion(
        completion_result: SessionCompletionResult,
        sessions: &Arc<AtomicLock<Sessions>>,
        symbols: &[Symbol],
        fee_calculator: BinanceFeeCalculator,
        observer: &Arc<AtomicLock<SingleObserver<OrderConnectorNotification>>>,
    ) {
        match completion_result {
            SessionCompletionResult::Success(credentials) => {
                tracing::info!(
                    "Session {} completed successfully",
                    credentials.into_session_id()
                );
                // Session completed normally, no action needed
            }
            SessionCompletionResult::Error {
                error,
                credentials,
                session_id,
            } => {
                tracing::warn!("Session {} terminated with error: {}", session_id, error);

                // Only attempt reconnection if error should trigger it and we have credentials
                if error.should_reconnect() {
                    if let Some(creds) = credentials {
                        Self::attempt_reconnection(
                            creds,
                            sessions,
                            symbols,
                            fee_calculator,
                            observer,
                            &session_id,
                        )
                        .await;
                    }
                }
            }
        }
    }

    async fn attempt_reconnection(
        credentials: Credentials,
        sessions: &Arc<AtomicLock<Sessions>>,
        symbols: &[Symbol],
        fee_calculator: BinanceFeeCalculator,
        observer: &Arc<AtomicLock<SingleObserver<OrderConnectorNotification>>>,
        original_session_id: &SessionId,
    ) {
        tracing::info!(
            "Attempting to recreate session {} after error",
            original_session_id
        );

        match sessions.write().add_session(
            credentials,
            symbols.to_vec(),
            fee_calculator,
            observer.clone(),
        ) {
            Ok(_) => {
                tracing::info!(
                    "Successfully recreated session {} after error",
                    original_session_id
                );
                // Note: Session will publish its own SessionLogon event, no need to publish here
            }
            Err(err) => {
                tracing::error!(
                    "Failed to recreate session {}: {:?}",
                    original_session_id,
                    err
                );
            }
        }
    }
}
