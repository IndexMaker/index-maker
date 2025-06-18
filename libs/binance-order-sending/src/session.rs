use std::sync::Arc;

use async_core::async_loop::AsyncLoop;
use eyre::{eyre, Report, Result};
use index_maker::order_sender::order_connector::SessionId;
use index_maker::{
    core::functional::{PublishSingle, SingleObserver},
    order_sender::order_connector::OrderConnectorNotification,
};
use itertools::Either;
use parking_lot::RwLock as AtomicLock;
use tokio::task::JoinError;
use tokio::{
    select,
    sync::mpsc::{UnboundedReceiver, UnboundedSender},
};

use crate::command::Command;
use crate::trading_session::TradingSessionBuilder;

pub struct Credentials {
    api_key: String,
    get_secret_fn: Box<dyn Fn() -> String + Send + Sync>,
}

impl Credentials {
    pub fn new(
        api_key: String,
        get_secret_fn: impl Fn() -> String + Send + Sync + 'static,
    ) -> Self {
        Self {
            api_key,
            get_secret_fn: Box::new(get_secret_fn),
        }
    }

    pub fn get_api_key(&self) -> String {
        self.api_key.clone()
    }

    pub(crate) fn get_api_secret(&self) -> String {
        (*self.get_secret_fn)()
    }

    pub(crate) fn into_session_id(&self) -> SessionId {
        SessionId(self.get_api_key())
    }
}

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

            let trading_session = match TradingSessionBuilder::build(&credentials).await {
                Err(err) => {
                    observer
                        .read()
                        .publish_single(OrderConnectorNotification::SessionLogout {
                            session_id,
                            reason: format!("Failed create session: {:?}", err),
                        });
                    return credentials;
                }
                Ok(s) => s,
            };

            if let Err(err) = trading_session.logon().await {
                observer
                    .read()
                    .publish_single(OrderConnectorNotification::SessionLogout {
                        session_id,
                        reason: format!("Failed to login: {:?}", err),
                    });
                return credentials;
            }

            let user_data = match trading_session.subscribe(observer.clone()).await {
                Err(err) => {
                    observer
                        .read()
                        .publish_single(OrderConnectorNotification::SessionLogout {
                            session_id: session_id.clone(),
                            reason: format!("Failed to obtain user-data: {:?}", err),
                        });
                    return credentials;
                }
                Ok(s) => s,
            };

            loop {
                select! {
                    _ = cancel_token.cancelled() => {
                        break;
                    },
                    Some(command) = command_rx.recv() => {
                        if let Err(res) = trading_session.send_command(command).await {
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
                    reason: "Session disconnected".to_owned(),
                });

            tracing::info!("Session loop exited");
            credentials
        });

        Ok(())
    }
}
