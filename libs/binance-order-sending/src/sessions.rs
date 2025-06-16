use std::{
    collections::{hash_map::Entry, HashMap},
    sync::Arc,
};

use eyre::{eyre, OptionExt, Result};
use index_maker::{
    core::functional::SingleObserver, order_sender::order_connector::OrderConnectorNotification,
};
use parking_lot::RwLock as AtomicLock;
use tokio::sync::mpsc::unbounded_channel;

use crate::{
    command::SessionCommand,
    session::{Credentials, Session},
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
        observer: Arc<AtomicLock<SingleObserver<OrderConnectorNotification>>>,
    ) -> Result<()> {
        match self.sessions.entry(credentials.api_key.clone()) {
            Entry::Vacant(entry) => {
                let (tx, rx) = unbounded_channel();
                let session = entry.insert(Session::new(tx));
                session.start(rx, observer, credentials)
            }
            Entry::Occupied(_) => Err(eyre!("Session already started")),
        }
    }

    pub fn remove_session(&mut self, api_key: &String) -> Option<Session> {
        self.sessions.remove(api_key)
    }

    pub fn send_command(&self, command: SessionCommand) -> Result<()> {
        let session = self
            .sessions
            .get(&command.api_key)
            .ok_or_eyre("Failed to find session")?;

        session.send_command(command.command)
    }
}
