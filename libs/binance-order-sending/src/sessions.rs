use std::{
    collections::{hash_map::Entry, HashMap},
    sync::Arc,
};

use eyre::{eyre, Result};
use futures_util::future::join_all;
use itertools::Itertools;
use parking_lot::RwLock as AtomicLock;
use symm_core::{
    core::functional::SingleObserver,
    order_sender::order_connector::{OrderConnectorNotification, SessionId},
};
use tokio::sync::mpsc::unbounded_channel;

use crate::{credentials::Credentials, session::Session};

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
        observer: Arc<AtomicLock<SingleObserver<OrderConnectorNotification>>>,
    ) -> Result<()> {
        match self.sessions.entry(credentials.into_session_id()) {
            Entry::Vacant(entry) => {
                let (tx, rx) = unbounded_channel();
                let session = entry.insert(Session::new(tx));
                session.start(rx, observer, credentials)
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
        self.sessions.drain().map(|(k, v)| v).collect_vec()
    }

    pub async fn stop_all(mut sessions: Vec<Session>) -> Result<()> {
        let stop_futures = sessions.iter_mut().map(|sess| sess.stop()).collect_vec();

        let (_, failures): (Vec<_>, Vec<_>) =
            join_all(stop_futures).await.into_iter().partition_result();

        if !failures.is_empty() {
            Err(eyre!(
                "Sessions join failed {}",
                failures.iter().map(|e| format!("{:?}", e)).join(";"),
            ))?;
        }

        Ok(())
    }
}
