use std::{
    collections::{HashMap, VecDeque},
    sync::Arc,
};

use chrono::Utc;
use eyre::Report;
use itertools::Either;
use parking_lot::RwLock as AtomicLock;
use symm_core::{
    core::{
        async_loop::AsyncLoop,
        bits::Symbol,
        functional::{OneShotPublishSingle, PublishSingle, SingleObserver},
    },
    order_sender::order_connector::{OrderConnectorNotification, SessionId},
};
use tokio::{
    select,
    sync::mpsc::{error::TryRecvError, UnboundedReceiver},
    task::JoinError,
};

use crate::{
    binance_order_sending::BinanceFeeCalculator,
    command::{self, Command},
    credentials::{self, Credentials},
    session_completion::SessionCompletionResult,
    sessions::Sessions,
    subaccounts::SubAccounts,
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
        // TODO: Configure me!
        let mut check_period = tokio::time::interval(std::time::Duration::from_secs(3));

        self.arbiter_loop.start(async move |cancel_token| {
            tracing::info!("Loop started");
            loop {
                select! {
                    _ = cancel_token.cancelled() => {
                        break
                    },
                    _ = check_period.tick() => {
                        let lost_sessions_ok = sessions.write().check_stopped();
                        match lost_sessions_ok {
                            Ok(lost_sessions) => {
                                for mut session in lost_sessions {
                                    let session_ok = session.stop().await;
                                    match session_ok {
                                        Ok((command_rx, completition)) => {
                                            Self::process_session_completion(
                                                command_rx,
                                                completition,
                                                true,
                                                &sessions,
                                                &symbols,
                                                fee_calculator.clone(),
                                                &observer,
                                            ).await;
                                        },
                                        Err(err) => {
                                            tracing::warn!("Failed to join stopped session: {:?}", err);   
                                        }
                                    }
                                }
                            }
                            Err(err) => {
                                tracing::warn!("Error while watching subscriptions {:?}", err);
                            }
                        }
                    }
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

            let all_sessions = sessions.write().drain_all_sessions();
            let session_completitions = Sessions::stop_all(all_sessions).await;

            for (command_rx, completition) in session_completitions {
                Self::process_session_completion(
                    command_rx,
                    completition,
                    false,
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
        mut command_rx: UnboundedReceiver<Command>,
        completion_result: SessionCompletionResult,
        should_attempt_reconnection: bool,
        sessions: &Arc<AtomicLock<Sessions>>,
        symbols: &[Symbol],
        fee_calculator: BinanceFeeCalculator,
        observer: &Arc<AtomicLock<SingleObserver<OrderConnectorNotification>>>,
    ) {
        // We must drain all unprocessed commands and then either re-send them
        // to new session or reply to them.
        let mut commands = VecDeque::new();
        loop {
            match command_rx.try_recv() {
                Ok(command) => {
                    tracing::debug!("Drained command: {:?}", command);
                    commands.push_back(command);
                }
                Err(TryRecvError::Empty) => {
                    tracing::info!("No more commants: All commands drained");
                    break;
                }
                Err(TryRecvError::Disconnected) => {
                    tracing::warn!("Failed to drain commands: Channel disconnected");
                    break;
                }
            }
        }

        let commands = match completion_result {
            SessionCompletionResult::Success(credentials) => {
                tracing::info!(
                    "Session {} completed successfully",
                    credentials.account_name()
                );
                commands
            }
            SessionCompletionResult::Error {
                error,
                credentials,
                session_id,
            } => {
                tracing::warn!("Session {} terminated with error: {}", session_id, error);

                match (
                    credentials,
                    should_attempt_reconnection && error.should_reconnect(),
                ) {
                    (Some(credentials), true) => {
                        // When we try reconnection, we should attempt to
                        // re-send all unprocessed commands to new session
                        Self::attempt_reconnection(
                            credentials,
                            commands,
                            sessions,
                            symbols,
                            fee_calculator,
                            observer,
                        )
                        .await
                    }
                    _ => commands,
                }
            }
        };

        // We must reply to all unprocessed commands, otherwise client will be
        // waiting indefinitely for reply
        for command in commands {
            match command {
                Command::EnableTrading(_) => {
                    tracing::warn!("Failed to enable trading: Session disconnected")
                }
                Command::NewOrder(single_order) => {
                    tracing::warn!("Failed to send new order: Session disconnected");
                    observer
                        .read()
                        .publish_single(OrderConnectorNotification::Rejected {
                            order_id: single_order.order_id.clone(),
                            symbol: single_order.symbol.clone(),
                            side: single_order.side,
                            price: single_order.price,
                            quantity: single_order.quantity,
                            reason: String::from("Sesssion disconnected"),
                            timestamp: Utc::now(),
                        });
                }
                Command::GetExchangeInfo(_) => {
                    tracing::warn!("Failed to obtain exchange info: Session disconnected")
                }
                Command::GetBalances(one_shot_single_observer) => {
                    tracing::warn!("Failed to obtain balances: Session disconnected");
                    one_shot_single_observer.one_shot_publish_single(HashMap::new());
                }
            }
        }
    }

    async fn attempt_reconnection(
        credentials: Credentials,
        commands: VecDeque<Command>,
        sessions: &Arc<AtomicLock<Sessions>>,
        symbols: &[Symbol],
        fee_calculator: BinanceFeeCalculator,
        observer: &Arc<AtomicLock<SingleObserver<OrderConnectorNotification>>>,
    ) -> VecDeque<Command> {
        let account_name = credentials.account_name().to_owned();

        tracing::info!(
            "Attempting to recreate session {} after error",
            account_name
        );

        match sessions.write().add_session(
            credentials,
            symbols.to_vec(),
            fee_calculator,
            observer.clone(),
        ) {
            Ok(session) => {
                tracing::info!(
                    "Successfully recreated session {} after error",
                    account_name
                );

                let mut failed_commands = VecDeque::new();

                for command in commands {
                    if let Err(command) = session.send_command(command) {
                        tracing::warn!("Failed to resend command to session {}", account_name);
                        failed_commands.push_back(command);
                    }
                }

                failed_commands
            }
            Err(err) => {
                tracing::error!("Failed to recreate session {}: {:?}", account_name, err);
                commands
            }
        }
    }
}
