use std::sync::Arc;
use std::thread::{self, spawn};

use async_core::async_loop::AsyncLoop;
use chrono::Utc;
use eyre::{eyre, Report, Result};
use futures_util::StreamExt;
use index_maker::order_sender::order_connector::SessionId;
use index_maker::{
    core::{
        bits::Side,
        functional::{PublishSingle, SingleObserver},
    },
    order_sender::order_connector::OrderConnectorNotification,
};
use itertools::Either;
use parking_lot::RwLock as AtomicLock;
use rust_decimal::dec;
use tokio::task::JoinError;
use tokio::{
    select,
    sync::mpsc::{UnboundedReceiver, UnboundedSender},
};

use crate::command::Command;
use crate::trading_session::{self, TradingSession, TradingSessionBuilder};

pub struct Credentials {
    pub api_key: String,
    pub get_secret_fn: Box<dyn Fn() -> String + Send + Sync>,
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
            println!("(binance-session) Logon");
            let session_id = SessionId(credentials.api_key.clone());

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
                            eprintln!("(binance-order-sending-session) Failed to send command: {:?}", res);
                        }
                    },
                }
            }

            user_data.unsubscribe().await;

            println!("(binance-session) Logout");
            observer
                .read()
                .publish_single(OrderConnectorNotification::SessionLogout {
                    session_id,
                    reason: "Session disconnected".to_owned(),
                });

            credentials
        });

        Ok(())
    }
}
