use std::{
    collections::{hash_map::Entry, HashMap},
    sync::Arc,
};

use eyre::{eyre, Result};
use futures_util::future::join_all;
use itertools::Itertools;
use parking_lot::RwLock as AtomicLock;
use symm_core::{
    core::{bits::Symbol, functional::SingleObserver},
    order_sender::order_connector::{OrderConnectorNotification, SessionId},
};
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver};

use crate::{
    binance_order_sending::BinanceFeeCalculator,
    command::Command,
    credentials::Credentials,
    session::{self, Session},
    session_completion::SessionCompletionResult,
};

pub struct Sessions {
    sessions: HashMap<SessionId, Session>,
}

impl Sessions {
    pub fn new() -> Self {
        Self {
            sessions: HashMap::new(),
        }
    }

    pub fn add_session(
        &mut self,
        credentials: Credentials,
        symbols: Vec<Symbol>,
        fee_calculator: BinanceFeeCalculator,
        observer: Arc<AtomicLock<SingleObserver<OrderConnectorNotification>>>,
    ) -> Result<&mut Session> {
        match self.sessions.entry(credentials.into_session_id()) {
            Entry::Vacant(entry) => {
                let (tx, rx) = unbounded_channel();
                let session = entry.insert(Session::new(tx));
                session.start(rx, observer, credentials, symbols, fee_calculator)?;
                Ok(session)
            }
            Entry::Occupied(_) => Err(eyre!("Session already started")),
        }
    }

    pub fn remove_session(&mut self, session_id: &SessionId) -> Option<Session> {
        self.sessions.remove(session_id)
    }

    pub fn get_session(&self, session_id: &SessionId) -> Option<&Session> {
        self.sessions.get(session_id)
    }

    pub fn drain_all_sessions(&mut self) -> Vec<Session> {
        self.sessions.drain().map(|(_k, v)| v).collect_vec()
    }

    pub async fn stop_all(
        mut sessions: Vec<Session>,
    ) -> Vec<(UnboundedReceiver<Command>, SessionCompletionResult)> {
        let stop_futures = sessions.iter_mut().map(|sess| sess.stop()).collect_vec();

        let (results, failures): (Vec<_>, Vec<_>) =
            join_all(stop_futures).await.into_iter().partition_result();

        if !failures.is_empty() {
            tracing::warn!(
                "Some sessions failed to stop cleanly: {}",
                failures.iter().map(|e| format!("{:?}", e)).join(";"),
            );
        }

        results
    }

    pub fn check_stopped(&mut self) -> Result<Vec<Session>> {
        let mut lost = Vec::new();

        self.sessions.retain(|session_id, session| {
            if session.has_stopped() {
                lost.push(session_id.clone());
                false
            } else {
                true
            }
        });

        let lost = lost
            .into_iter()
            .filter_map(|k| self.sessions.remove(&k))
            .collect_vec();

        Ok(lost)
    }
}
