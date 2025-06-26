use std::sync::Arc;

use chrono::Utc;
use eyre::{eyre, Report, Result};
use itertools::Either;
use parking_lot::RwLock as AtomicLock;
use symm_core::{
    core::{
        async_loop::AsyncLoop,
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

pub struct Session {
    command_tx: UnboundedSender<Command>,
    session_loop: AsyncLoop<Credentials>,
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

    pub async fn stop(&mut self) -> Result<Credentials, Either<JoinError, Report>> {
        self.session_loop.stop().await
    }

    pub fn start(
        &mut self,
        mut command_rx: UnboundedReceiver<Command>,
        observer: Arc<AtomicLock<SingleObserver<OrderConnectorNotification>>>,
        credentials: Credentials,
    ) -> Result<()> {
        self.session_loop.start(async move |cancel_token| {
            tracing::info!("Session loop started");
            let session_id = credentials.into_session_id();
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
                    on_error(format!("Failed create session: {:?}", err));
                    return credentials;
                }
                Ok(s) => s,
            };

            if let Err(err) = trading_session.logon().await {
                on_error(format!("Failed to login: {:?}", err));
                return credentials;
            }

            let user_data = match trading_session.subscribe(observer.clone()).await {
                Err(err) => {
                    on_error(format!("Failed to obtain user-data: {:?}", err));
                    return credentials;
                }
                Ok(s) => s,
            };

            observer
                .read()
                .publish_single(OrderConnectorNotification::SessionLogon {
                    session_id: session_id.clone(),
                    timestamp: Utc::now(),
                });

            loop {
                select! {
                    _ = cancel_token.cancelled() => {
                        break;
                    },
                    Some(command) = command_rx.recv() => {
                        if let Err(res) = trading_session.send_command(command, &observer).await {
                            tracing::warn!("Failed to send command: {:?}", res);
                        }
                    },
                }
            }

            user_data.unsubscribe().await;

            observer
                .read()
                .publish_single(OrderConnectorNotification::SessionLogout {
                    session_id,
                    reason: "Session ended".to_owned(),
                    timestamp: Utc::now(),
                });

            tracing::info!("Session loop exited");
            credentials
        });

        Ok(())
    }
}
