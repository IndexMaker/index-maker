use std::sync::Arc;

use eyre::Report;
use itertools::Either;
use parking_lot::RwLock as AtomicLock;
use symm_core::{
    core::{async_loop::AsyncLoop, bits::Symbol, functional::SingleObserver},
    order_sender::order_connector::OrderConnectorNotification,
};
use tokio::{select, sync::mpsc::UnboundedReceiver, task::JoinError};

use crate::{credentials::Credentials, sessions::Sessions, subaccounts::SubAccounts};

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
            let sessions = sessions.write().drain_all_sessions();
            if let Err(err) = Sessions::stop_all(sessions).await {
                tracing::warn!("Error stopping sessions {:?}", err);
            }
            tracing::info!("Loop exited");
            subaccount_rx
        });
    }
}
