use std::sync::Arc;

use async_core::async_loop::AsyncLoop;
use binance_spot_connector_rust::http::Credentials;
use eyre::Report;
use index_maker::{
    core::functional::SingleObserver, order_sender::order_connector::OrderConnectorNotification,
};
use itertools::Either;
use parking_lot::RwLock as AtomicLock;
use tokio::{select, sync::mpsc::UnboundedReceiver, task::JoinError};

use crate::{command::SessionCommand, sessions::Sessions, subaccounts::SubAccounts};

/// Arbiter manages open sessions
/// 
/// When started it receives session credentials, and then
/// opens connection and passes credentials for logon. After
/// that session is running, and arbiter is forwarding session
/// commands to target sessions.
pub struct Arbiter {
    arbiter_loop: AsyncLoop<(
        UnboundedReceiver<Credentials>,
        UnboundedReceiver<SessionCommand>,
    )>,
}

impl Arbiter {
    pub fn new() -> Self {
        Self {
            arbiter_loop: AsyncLoop::new(),
        }
    }

    pub async fn stop(
        &mut self,
    ) -> Result<
        (
            UnboundedReceiver<Credentials>,
            UnboundedReceiver<SessionCommand>,
        ),
        Either<JoinError, Report>,
    > {
        self.arbiter_loop.stop().await
    }

    pub fn start(
        &mut self,
        subaccounts: Arc<AtomicLock<SubAccounts>>,
        mut subaccount_rx: UnboundedReceiver<Credentials>,
        mut command_rx: UnboundedReceiver<SessionCommand>,
        observer: Arc<AtomicLock<SingleObserver<OrderConnectorNotification>>>,
    ) {
        let mut sessions = Sessions::new();
        self.arbiter_loop.start(async move |cancel_token| {
            loop {
                select! {
                    _ = cancel_token.cancelled() => {
                        break
                    },
                    Some(credentials) = subaccount_rx.recv() => {
                        let api_key = credentials.api_key.clone();
                        match sessions.add_session(credentials, observer.clone()).await {
                            Ok(_) => {
                                let mut suba = subaccounts.write();
                                if let Err(err) = suba.add_subaccount_taken(api_key) {
                                    eprintln!("Error storing taken session {:?}", err);
                                }
                            }
                            Err(err) => {
                                eprintln!("Error while creating session {:?}", err);
                            }
                        }
                    },
                    Some(command) = command_rx.recv() => {
                        match sessions.send_command(command).await {
                            Ok(_) => {},
                            Err(err) => {
                                eprintln!("Error while sending session command {:?}", err);
                            }
                        }
                    }
                }
            }
            (subaccount_rx, command_rx)
        });
    }
}
