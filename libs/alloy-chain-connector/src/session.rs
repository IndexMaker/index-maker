use std::{collections::HashMap, sync::Arc};

use alloy_primitives::B256;
use chrono::Utc;
use eyre::{eyre, Report, Result};
use index_core::blockchain::chain_connector::ChainNotification;
use itertools::Either;
use otc_custody::{custody_client::CustodyClient, index::index::IndexInstance};
use parking_lot::RwLock as AtomicLock;
use symm_core::core::{
    async_loop::AsyncLoop,
    bits::{Address, Symbol},
    functional::{OneShotPublishSingle, PublishSingle, SingleObserver},
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

#[derive(Clone)]
pub struct SessionBaggage {
    pub custody_clients: Arc<AtomicLock<HashMap<B256, CustodyClient>>>,
    pub indexes_by_symbol: Arc<AtomicLock<HashMap<Symbol, Arc<IndexInstance>>>>,
    pub indexes_by_address: Arc<AtomicLock<HashMap<Address, Arc<IndexInstance>>>>,
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
        observer: Arc<AtomicLock<SingleObserver<ChainNotification>>>,
        credentials: Credentials,
        baggage: SessionBaggage,
    ) -> Result<()> {
        let account_name = credentials.get_account_name();
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

            tracing::info!(%account_name, %chain_id, %signer_address, "Session loop started");

            let providers = match credentials.connect_any().await {
                Ok(ok) => ok,
                Err(err) => {
                    on_error(format!("Failed to connect session: {:?}", err));
                    return credentials;
                }
            };

            let ws_providers = match credentials.connect_any_ws().await {
                Ok(ok) => ok,
                Err(err) => {
                    on_error(format!("Failed to connect public session: {:?}", err));
                    return credentials;
                }
            };

            let public_providers = if ws_providers.is_empty() {
                match credentials.connect_any_public().await {
                    Ok(ok) => Some(ok),
                    Err(err) => {
                        on_error(format!("Failed to connect public session: {:?}", err));
                        return credentials;
                    }
                }
            } else {
                None
            };

            let mut rpc_basic_session = RpcBasicSession::new(account_name.clone(), providers.clone());

            let mut rpc_issuer_session = RpcIssuerSession::new(account_name.clone(), providers.clone());
            let mut rpc_custody_session = RpcCustodySession::new(account_name.clone(), providers.clone());

            observer
                .read()
                .publish_single(ChainNotification::ChainConnected {
                    chain_id,
                    timestamp: Utc::now(),
                });

            let rpc_issuer_stream = if !ws_providers.is_empty() {
                let mut s = RpcIssuerStream::new(account_name.clone(), ws_providers);

                if let Err(err) = s.subscribe_streaming(
                        chain_id, baggage.indexes_by_address.clone(), observer.clone()
                    ).await
                {
                    on_error(format!("Failed to subscribe to RPC events: {:?}", err));
                    return credentials;
                }
                Some(s)
            } else {
                None
            };

            let rpc_issuer_stream1 = if let Some(public_providers) = public_providers {
                let mut s = RpcIssuerStream::new(account_name.clone(), public_providers);

                if let Err(err) = s.subscribe_polling(
                        chain_id, baggage.indexes_by_address, observer.clone()
                    ).await
                {
                    on_error(format!("Failed to subscribe to RPC events: {:?}", err));
                    return credentials;
                }
                Some(s)
            } else {
                None
            };

            loop {
                select! {
                    _ = cancel_token.cancelled() => {
                        break;
                    },
                    Some(command) = command_rx.recv() => {
                        if let Err(err) = match command.command {
                            CommandVariant::Basic{ contract_address, command }=> {
                                rpc_basic_session.send_basic_command(contract_address, command).await
                            }
                            CommandVariant::Issuer{symbol, command} => {
                                let maybe_index = baggage.indexes_by_symbol.read().get(&symbol).cloned();
                                if let Some(index) = maybe_index {
                                    rpc_issuer_session.send_issuer_command(index, command).await
                                } else {
                                    Err(eyre!("Index not found: {}", symbol))
                                }
                            }
                            CommandVariant::Custody{custody_id, token, command} => {
                                let maybe_custody_client = baggage.custody_clients.read().get(&custody_id).cloned();
                                if let Some(custody_client) = maybe_custody_client {
                                    rpc_custody_session.send_custody_command(
                                        custody_client, token, command).await
                                } else {
                                    Err(eyre!("Custody not found: {}", custody_id))
                                }
                            }
                        } {
                            tracing::warn!(%account_name, "Command failed: {:?}", err);
                            command.error_observer.one_shot_publish_single(err);
                        }
                    },
                }
            }

            if let Some(mut rpc_issuer_stream) = rpc_issuer_stream {
                if let Err(err) = rpc_issuer_stream.unsubscribe().await {
                    tracing::warn!("Failed to unsubscribe from RPC events: {:?}", err);
                }
            }
            if let Some(mut rpc_issuer_stream) = rpc_issuer_stream1 {
                if let Err(err) = rpc_issuer_stream.unsubscribe().await {
                    tracing::warn!("Failed to unsubscribe from RPC events: {:?}", err);
                }
            }

            observer
                .read()
                .publish_single(ChainNotification::ChainDisconnected {
                    chain_id,
                    reason: String::from("Session ended"),
                    timestamp: Utc::now(),
                });

            tracing::info!(%account_name, %chain_id, %signer_address, "Session loop exited");
            credentials
        });

        Ok(())
    }
}
