use std::sync::Arc;

use chrono::Utc;
use eyre::{eyre, Report, Result};
use index_core::blockchain::chain_connector::ChainNotification;
use itertools::Either;
use parking_lot::RwLock as AtomicLock;
use symm_core::core::{
    async_loop::AsyncLoop,
    bits::Amount,
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
    util::amount_converter::AmountConverter,
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

            let rpc_basic_session = match RpcBasicSession::try_new(provider.clone(), signer_address).await {
                Ok(x) => x,
                Err(err) => {
                    on_error(format!("Failed to create basic RPC session: {:?}", err));
                    return credentials;
                },
            };

            let converter = rpc_basic_session.get_amount_converter();

            let rpc_issuer_session = RpcIssuerSession::new(provider.clone(), signer_address, converter.clone());
            let rpc_issuer_stream = RpcIssuerStream::new(provider.clone(), signer_address, converter.clone());

            let rpc_custody_session = RpcCustodySession::new(provider, signer_address, converter.clone());

            observer
                .read()
                .publish_single(ChainNotification::ChainConnected {
                    chain_id,
                    timestamp: Utc::now(),
                });

            loop {
                select! {
                    _ = cancel_token.cancelled() => {
                        break;
                    },
                    maybe_event = rpc_issuer_stream.next_event() => {
                        match maybe_event {
                            Ok(event) => {
                                observer.read().publish_single(event);
                            },
                            Err(err) => {
                                on_error(format!("Failed to receive RPC stream event: {:?}", err));
                                return credentials;
                            }
                        }
                    }
                    Some(command) = command_rx.recv() => {
                        if let Err(err) = match command.command {
                            CommandVariant::Basic(basic_command) => {
                                rpc_basic_session.send_basic_command(basic_command).await
                            }
                            CommandVariant::Issuer(issuer_command) => {
                                rpc_issuer_session.send_issuer_command(issuer_command).await
                            }
                            CommandVariant::Custody(custody_command) => {
                                rpc_custody_session.send_custody_command(custody_command).await
                            }
                        } {
                            tracing::warn!("Command failed: {:?}", err);
                            command.error_observer.publish_single(err);
                        }
                    },
                }
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
