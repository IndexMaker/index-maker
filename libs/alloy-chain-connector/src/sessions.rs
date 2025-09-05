use std::{
    collections::{hash_map::Entry, HashMap},
    sync::Arc,
};

use alloy_primitives::B256;
use eyre::{eyre, Result};
use futures_util::future::join_all;
use index_core::blockchain::chain_connector::ChainNotification;
use itertools::Itertools;
use otc_custody::{custody_client::CustodyClient, index::index::IndexInstance};
use parking_lot::RwLock as AtomicLock;
use symm_core::core::{
    bits::{Address, Symbol},
    functional::SingleObserver,
};
use tokio::sync::mpsc::unbounded_channel;

use crate::{
    credentials::Credentials,
    session::{Session, SessionBaggage},
};

pub struct Sessions {
    sessions: HashMap<String, Session>,
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
        baggage: SessionBaggage,
        observer: Arc<AtomicLock<SingleObserver<ChainNotification>>>,
    ) -> Result<()> {
        match self.sessions.entry(credentials.get_account_name()) {
            Entry::Vacant(entry) => {
                let (tx, rx) = unbounded_channel();
                let session = entry.insert(Session::new(tx));
                session.start(rx, observer, credentials, baggage)
            }
            Entry::Occupied(_) => Err(eyre!("Session already started")),
        }
    }

    pub fn remove_session(&mut self, account_name: &str) -> Option<Session> {
        self.sessions.remove(account_name)
    }

    pub fn get_session(&self, account_name: &str) -> Option<&Session> {
        self.sessions.get(account_name)
    }

    pub fn drain_all_sessions(&mut self) -> Vec<Session> {
        self.sessions.drain().map(|(_, v)| v).collect_vec()
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
