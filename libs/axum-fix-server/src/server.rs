use std::{
    collections::{hash_map::Entry, HashMap},
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Weak,
    },
};

use axum::{
    extract::{
        ws::{Message, WebSocket},
        State, WebSocketUpgrade,
    },
    response::IntoResponse,
    routing::get,
    Router,
};
use eyre::{eyre, Result};
use std::net::SocketAddr;
use tokio::{
    select,
    sync::{
        mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender},
        RwLock,
    },
};

use crate::{
    messages::{ServerResponse, SessionId},
    server_plugin::ServerPlugin,
};

/// Session
///
/// Manages a single client session for sending responses. Holds a sender channel for
/// responses and a unique session identifier.
pub struct Session {
    response_tx: UnboundedSender<String>,
}

impl Session {
    /// new
    ///
    /// Creates a new Session.
    pub fn new(tx: UnboundedSender<String>) -> Self {
        Self { response_tx: tx }
    }

    /// send_response
    ///
    /// Sends a response to the client by enqueueing a message
    /// to the receivieng channel held by 'ws_handler'.
    /// Returns a `Result` indicating success or failure.
    pub fn send_response(&self, response: String) -> Result<()> {
        self.response_tx
            .send(response)
            .map_err(|err| eyre!("Error {:?}", err))
    }
}

/// Server
///
/// The core server structure that manages client sessions and handles incoming requests,
/// it maintains a map of all active sessions and handles their lifetime. The server
/// must be loaded with a `Plugin` to define incoming and outgoing message handling.
/// The `Plugin` consumes the message and is responsible for deserialization,
/// message validation (fields, seqnum, signatures), publishing to application, etc.
pub struct Server<Q, P>
where
    Q: ServerResponse,
    P: ServerPlugin<Q>,
{
    me: Weak<RwLock<Self>>,
    sessions: HashMap<SessionId, Arc<Session>>,
    session_id_counter: AtomicUsize,
    plugin: P,
    pub accept_connections: bool,
}

impl<Q, P> Server<Q, P>
where
    Q: ServerResponse + Send + Clone + 'static,
    P: ServerPlugin<Q> + Send + Sync + 'static,
{
    /// new
    ///
    /// Creates a new instance of `Server`, an empty map of sessions, and a new observer.
    /// Initializes the session ID counter to start from 1.
    pub fn new_arc(plugin: P) -> Arc<RwLock<Self>> {
        Arc::new_cyclic(|me| {
            RwLock::new(Self {
                me: me.clone(),
                sessions: HashMap::new(),
                session_id_counter: AtomicUsize::new(1),
                plugin,
                accept_connections: true,
            })
        })
    }

    /// start_server
    ///
    /// Initializes the server, spawining it on a thread using `ws_handler` logic.
    pub fn start_server(&self, address: &'static str) {
        let my_clone = self.me.upgrade().unwrap();
        tokio::spawn(async move {
            let addr: SocketAddr = address.parse().expect(&format!(
                "Server failed to start: Invalid address ({})",
                address
            ));
            tracing::info!("Listening on {}", addr);

            let app = Router::new()
                .route("/ws", get(ws_handler))
                .with_state(my_clone);

            if let Err(e) = axum_server::bind(addr).serve(app.into_make_service()).await {
                tracing::warn!("Server failed to start: {}", e);
            }
        });
    }

    /// close_server
    ///
    /// Closes server for new connections
    pub fn close_server(&mut self) {
        self.accept_connections = false;
    }

    /// close_server
    ///
    /// Closes all sessions
    pub fn stop_server(&mut self) {
        let session_ids: Vec<_> = self.sessions.keys().cloned().collect();
        for session_id in session_ids {
            self.close_session(&session_id);
        }
    }

    /// create_session
    ///
    /// Creates a new client session with a unique session ID. Returns a tuple containing
    /// the session receiver channel (for sending response buffers), and the session ID.
    pub fn create_session(&mut self) -> Result<(UnboundedReceiver<String>, SessionId)> {
        let session_id = SessionId(format!(
            "session_{}",
            self.session_id_counter.fetch_add(1, Ordering::SeqCst)
        ));
        match self.sessions.entry(session_id.clone()) {
            Entry::Occupied(_) => {
                let error_msg = format!("Session already opened: {}", &session_id);
                tracing::warn!(error_msg);
                Err(eyre!(error_msg))
            }
            Entry::Vacant(vacant_entry) => {
                let (tx, rx) = unbounded_channel();
                vacant_entry.insert(Arc::new(Session::new(tx)));
                match self.plugin.create_session(&session_id) {
                    Ok(()) => Ok((rx, session_id)),
                    Err(e) => Err(e),
                }
            }
        }
    }

    /// close_session
    ///
    /// Closes an existing client session with a unique session ID.
    pub fn close_session(&mut self, session_id: &SessionId) {
        match self.plugin.destroy_session(&session_id) {
            Ok(()) => {
                self.sessions.remove(&session_id);
            }
            Err(e) => {
                tracing::warn!("Failed to destroy session: {}; {}", &session_id, e);
            }
        }
    }

    /// receive_message
    ///
    /// Process incoming message by passing it to the `Plugin`. The `Plugin` will be responsible
    /// for deserialization, validation, and publishing the result to the application.
    pub fn receive_message(&self, message: String, session_id: &SessionId) -> Result<()> {
        self.plugin.process_incoming(message, session_id)
    }

    /// send_response
    ///
    /// Sends a response to the appropriate client session based on the session ID in the response.
    /// Returns a `Result` indicating success or failure if the session is not found.
    pub fn send_response(&self, response: Q) -> Result<()> {
        //let session_id = response.get_session_id().clone();
        if let Ok(session_responses) = self.plugin.process_outgoing(response) {
            for session_response in session_responses {
                match self.sessions.get(&session_response.0) {
                    Some(session) => {
                        if let Err(e) = session.send_response(session_response.1.clone()) {
                            tracing::error!("Failed to send response to session {}: {:?}", session_response.0, e);
                            return Err(e.into());
                        }
                    },
                    None => {
                        let error_msg = format!("Session not found: {}", &session_response.0);
                        tracing::warn!(error_msg);
                    }
                }
            }
            Ok(())
        } else {
            Err(eyre!("Unable to process response."))
        }
    }
}

/// ws_handler
///
/// This is the closure that contains the async logic used by the `Server`. On a WS upgrade it
/// creates a new session and awaits for either a message from the client (on the ws), or
/// for a response coming from the server (on the tokio channel). In case of an error, or the
/// session is closed by the client, the loop is broken and the session destroyed.
async fn ws_handler<Q, P>(
    ws: WebSocketUpgrade,
    State(server): State<Arc<RwLock<Server<Q, P>>>>,
) -> impl IntoResponse
where
    Q: ServerResponse + Send + Clone + 'static,
    P: ServerPlugin<Q> + Send + Sync + 'static,
{
    ws.on_upgrade(move |mut ws: WebSocket| async move {
        if server.read().await.accept_connections {
        let (mut receiver, session_id) = server.write().await.create_session().unwrap();
        loop {
            select! {
                res = ws.recv() => {
                    match res {
                        Some(Ok(message)) => {
                            if let axum::extract::ws::Message::Text(text) = message {
                                tracing::debug!("Received message on {}: {}", &session_id, &text);
                                // Server processes received message
                                if let Err(e) = server.read().await.receive_message(text.to_string(), &session_id) {
                                    // Session sends error message back to the client
                                    tracing::warn!("Failed to process incoming message: {}", e);
                                    if let Err(e) = ws.send(Message::Text(e.to_string())).await {
                                        tracing::warn!("Failed to send WebSocket message: {}", e);
                                        break;
                                    }
                                }
                            }
                        }
                        Some(Err(e)) => {
                            tracing::warn!("WebSocket error: {}", e);
                            break;
                        }
                        None => {
                            tracing::info!("WebSocket connection closed on session {}", &session_id);
                            break;
                        }
                    }
                }
                Some(res) = receiver.recv() => {
                    match ws.send(Message::Text(res)).await {
                        Ok(()) => {},
                        Err(e) => {
                            tracing::warn!("WebSocket error: {}", e);
                            break;
                        }
                    }
                }
            }
        }
        server.write().await.close_session(&session_id);
    }
    })
}
