use std::{
    collections::{hash_map::Entry, HashMap},
    sync::Arc,
};

use eyre::{eyre, OptionExt, Result};
use symm_core::{
    core::functional::SingleObserver,
    order_sender::order_connector::{OrderConnectorNotification, SessionId},
};
use parking_lot::RwLock as AtomicLock;
use tokio::sync::mpsc::unbounded_channel;

use crate::{
    command::SessionCommand, credentials::Credentials, session::Session
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

    pub fn send_command(&self, command: SessionCommand) -> Result<()> {
        let session = self
            .sessions
            .get(&command.session_id)
            .ok_or_eyre("Failed to find session")?;

        session.send_command(command.command)
    }
}
