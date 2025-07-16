use std::{
    collections::{hash_map::Entry, HashMap},
    marker::PhantomData,
    sync::Arc,
};

use eyre::{eyre, OptionExt, Result};
use itertools::Itertools;
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver};

use crate::{messages::SessionId, server_plugin::ServerPlugin, session::Session};

struct Sessions {
    sessions: HashMap<SessionId, Arc<Session>>,
    session_id_counter: usize,
}

impl Sessions {
    fn new() -> Self {
        Self {
            sessions: HashMap::new(),
            session_id_counter: 0,
        }
    }

    fn gen_next_session_id(&mut self) -> SessionId {
        self.session_id_counter += 1;
        SessionId::from(format!("S-{}", self.session_id_counter))
    }
}

pub struct ServerState<Response, Plugin> {
    _phantom_response: PhantomData<Response>,
    sessions: Sessions,
    plugin: Plugin,
    accept_connections: bool,
}

impl<Response, Plugin> ServerState<Response, Plugin>
where
    Response: Send + 'static,
    Plugin: ServerPlugin<Response> + Send + Sync + 'static,
{
    pub fn new(plugin: Plugin) -> Self {
        Self {
            _phantom_response: PhantomData::default(),

            sessions: Sessions::new(),
            plugin,
            accept_connections: true,
        }
    }

    pub fn is_accepting_connections(&self) -> bool {
        self.accept_connections
    }

    pub fn close(&mut self) {
        self.accept_connections = false;
    }

    pub fn get_plugin(&self) -> &Plugin {
        &self.plugin
    }

    pub fn get_plugin_mut(&mut self) -> &mut Plugin {
        &mut self.plugin
    }

    pub fn get_session_ids(&self) -> Vec<SessionId> {
        self.sessions.sessions.keys().cloned().collect_vec()
    }
    pub fn create_session(&mut self) -> Result<(UnboundedReceiver<String>, Arc<Session>)> {
        let session_id = self.sessions.gen_next_session_id();

        match self.sessions.sessions.entry(session_id.clone()) {
            Entry::Occupied(_) => Err(eyre!("Session {} already exists", session_id)),
            Entry::Vacant(vacant_entry) => {
                let (tx, rx) = unbounded_channel();
                let session = vacant_entry.insert(Arc::new(Session::new(session_id, tx)));
                match self.plugin.create_session(session.get_session_id()) {
                    Ok(()) => Ok((rx, session.clone())),
                    Err(e) => Err(e),
                }
            }
        }
    }

    pub fn close_session(&mut self, session_id: SessionId) -> Result<Arc<Session>> {
        self.plugin.destroy_session(&session_id)?;
        let session = self
            .sessions
            .sessions
            .remove(&session_id)
            .ok_or_eyre("Session no longer exists")?;

        session.will_stop();
        Ok(session)
    }

    pub fn close_all_sessions(&mut self) -> Result<Vec<Arc<Session>>> {
        let (good, bad): (Vec<_>, Vec<_>) = self
            .get_session_ids()
            .into_iter()
            .map(|session_id| self.close_session(session_id))
            .partition_result();

        if !bad.is_empty() {
            Err(eyre!(
                "Failed to close sessions: {}",
                bad.into_iter().map(|err| format!("{:?}", err)).join(",")
            ))?;
        }

        Ok(good)
    }

    pub fn process_incoming(&self, message: String, session_id: &SessionId) -> Result<String> {
        tracing::debug!("Received message on {}: {}", session_id, message);
        self.plugin.process_incoming(message, session_id)
    }

    pub fn process_outgoing(&self, response: Response) -> Result<()> {
        let session_responses = self.plugin.process_outgoing(response)?;

        let (_, bad): ((), Vec<_>) = session_responses
            .into_iter()
            .map(|(sid, resp)| {
                let session = self
                    .sessions
                    .sessions
                    .get(&sid)
                    .ok_or_else(|| eyre!("Session {} not found", sid))?;

                session.send_response(resp)
            })
            .partition_result();

        if !bad.is_empty() {
            Err(eyre!(
                "Failed to send response: {}",
                bad.into_iter().map(|err| format!("{:?}", err)).join(", ")
            ))?;
        }

        Ok(())
    }
}
