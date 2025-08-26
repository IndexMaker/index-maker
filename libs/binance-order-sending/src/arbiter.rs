use std::sync::Arc;

use eyre::Report;
use itertools::Either;
use parking_lot::RwLock as AtomicLock;
use symm_core::{
    core::{async_loop::AsyncLoop, bits::Symbol, functional::{PublishSingle, SingleObserver}},
    order_sender::order_connector::OrderConnectorNotification,
};
use tokio::{select, sync::mpsc::UnboundedReceiver, task::JoinError};
use chrono::Utc;

use crate::{
    credentials::Credentials, 
    sessions::Sessions, 
    subaccounts::SubAccounts,
    session_termination::{SessionTerminationResult, SessionTerminationReason},
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
                        match sessions.write().add_session(credentials, symbols.clone(), observer.clone()) {
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
            
            // Enchanced shutdown with disconnection handling
            let all_sessions = sessions.write().drain_all_sessions();
            let termination_results = match Sessions::stop_all(all_sessions).await {
                Ok(res) => res,
                Err(err) => {
                    tracing::warn!("Error stopping sessions {:?}", err);
                    Vec::new()
                }
            };
            
            for result in termination_results {
                Self::handle_session_termination_result(
                    result,
                    &sessions,
                    &symbols,
                    &observer,
                    &subaccounts,
                ).await;
            }
            
            tracing::info!("Loop exited");
            subaccount_rx
        });
    }
    
    async fn handle_session_termination_result(
        termination_result: SessionTerminationResult,
        sessions: &Arc<AtomicLock<Sessions>>,
        symbols: &[Symbol],
        observer: &Arc<AtomicLock<SingleObserver<OrderConnectorNotification>>>,
        _subaccounts: &Arc<AtomicLock<SubAccounts>>,
    ) {
        match termination_result.reason {
            SessionTerminationReason::Graceful => {
                tracing::info!("Session {} terminated gracefully", termination_result.session_id);
            },
            SessionTerminationReason::WebSocketDisconnection => {
                tracing::warn!(
                    "Session {} disconnected",
                    termination_result.session_id
                );
                
                if let Some(credentials) = termination_result.credentials {
                    tracing::info!("Attempting to recreate session {} after disconnection", termination_result.session_id);
                    
                    match sessions.write().add_session(credentials, symbols.to_vec(), observer.clone()) {
                        Ok(_) => {
                            tracing::info!("Successfully recreated session {} after disconnection", termination_result.session_id);
                            
                        }
                        Err(err) => {
                            tracing::error!("Failed to recreate session {} after disconnection: {:?}", termination_result.session_id, err);
                            
                            observer.read().publish_single(
                                OrderConnectorNotification::SessionLogout { 
                                    session_id: termination_result.session_id,
                                    reason: format!("Disconnection recovery failed: {:?}", err),
                                    timestamp: Utc::now(),
                                }
                            );
                        }
                    }
                } else {
                    observer.read().publish_single(
                        OrderConnectorNotification::SessionLogout { 
                            session_id: termination_result.session_id,
                            reason: format!("Websocket disconnected"),
                            timestamp: Utc::now(),
                         }
                    );
                }
            },
            _ => {
                tracing::warn!(
                    "Session {} terminated: {:?}",
                    termination_result.session_id,
                    termination_result.reason
                );
                
                observer.read().publish_single(
                    OrderConnectorNotification::SessionLogout { 
                        session_id: termination_result.session_id,
                        reason: format!("Session terminated: {:?}", termination_result.reason),
                        timestamp: Utc::now(),
                    }
                );
            }
        }
    }
}
