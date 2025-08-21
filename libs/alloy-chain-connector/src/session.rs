use std::sync::Arc;

use chrono::Utc;
use eyre::{eyre, Report, Result};
use index_core::blockchain::chain_connector::ChainNotification;
use itertools::Either;
use parking_lot::RwLock as AtomicLock;
use symm_core::core::{
    async_loop::AsyncLoop,
    functional::{PublishSingle, SingleObserver},
};
use tokio::task::JoinError;
use tokio::{
    select,
    sync::mpsc::{UnboundedReceiver, UnboundedSender},
};

use crate::{
    command::{Command, CommandVariant},
    credentials::Credentials,
    rpc::{
        basic_session::RpcBasicSession, custody_session::RpcCustodySession,
        issuer_session::RpcIssuerSession, issuer_stream::RpcIssuerStream,
    },
};

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
        observer: Arc<AtomicLock<SingleObserver<ChainNotification>>>,
        credentials: Credentials,
    ) -> Result<()> {
        let chain_id = credentials.get_chain_id();
        let usdc_address = credentials.get_usdc_address();
        let signer_address = credentials.get_signer_address()?;

        self.session_loop.start(async move |cancel_token| {
            let on_error = |reason| {
                tracing::warn!(%chain_id, %signer_address, "{:?}", reason);
                observer
                    .read()
                    .publish_single(ChainNotification::ChainDisconnected {
                        chain_id,
                        reason,
                        timestamp: Utc::now(),
                    });
            };

            tracing::info!(%chain_id, %signer_address, "Session loop started");

            let provider = match credentials.connect().await {
                Ok(ok) => ok,
                Err(err) => {
                    on_error(format!("Failed to connect session: {:?}", err));
                    return credentials;
                }
            };

            let public_provider = match credentials.connect_public().await {
                Ok(ok) => ok,
                Err(err) => {
                    on_error(format!("Failed to connect public session: {:?}", err));
                    return credentials;
                }
            };

            let rpc_basic_session = RpcBasicSession::new(provider.clone());

            let rpc_issuer_session = RpcIssuerSession::new(provider.clone());
            let rpc_custody_session = RpcCustodySession::new(provider.clone());

            let mut rpc_issuer_stream = RpcIssuerStream::new(public_provider);

            observer
                .read()
                .publish_single(ChainNotification::ChainConnected {
                    chain_id,
                    timestamp: Utc::now(),
                });

            if let Err(err) = rpc_issuer_stream
                .subscribe(chain_id, usdc_address, observer.clone())
                .await
            {
                on_error(format!("Failed to subscribe to RPC events: {:?}", err));
                return credentials;
            }

            loop {
                select! {
                    _ = cancel_token.cancelled() => {
                        break;
                    },
                    Some(command) = command_rx.recv() => {
                        if let Err(err) = match command.command {
                            CommandVariant::Basic(basic_command) => {
                                rpc_basic_session.send_basic_command(
                                    command.contract_address, basic_command).await
                            }
                            CommandVariant::Issuer(issuer_command) => {
                                rpc_issuer_session.send_issuer_command(
                                    command.contract_address, issuer_command, usdc_address).await
                            }
                            CommandVariant::Custody(custody_command) => {
                                rpc_custody_session.send_custody_command(
                                    command.contract_address, custody_command, usdc_address).await
                            }
                        } {
                            tracing::warn!("Command failed: {:?}", err);
                            command.error_observer.publish_single(err);
                        }
                    },
                }
            }

            if let Err(err) = rpc_issuer_stream.unsubscribe().await {
                tracing::warn!("Failed to unsubscribe from RPC events: {:?}", err);
            }

            observer
                .read()
                .publish_single(ChainNotification::ChainDisconnected {
                    chain_id,
                    reason: String::from("Session ended"),
                    timestamp: Utc::now(),
                });

            tracing::info!(%chain_id, %signer_address, "Session loop exited");
            credentials
        });

        Ok(())
    }
}
