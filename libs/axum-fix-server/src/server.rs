use std::{
    collections::{hash_map::Entry, HashMap},
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Weak,
    },
    usize,
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
use eyre::{eyre, Report, Result};
use symm_core::core::functional::{PublishSingle, SingleObserver};
use std::net::SocketAddr;
use tokio::{
    select,
    sync::{
        mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender},
        RwLock,
    },
};

use crate::{
    messages::{ServerRequest, ServerResponse, SessionId},
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
        Self {
            response_tx: tx,
        }
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
/// The core server structure that manages client sessions and handles incoming requests.
/// It uses a `MultiObserver` to publish requests to registered observers and maintains
/// a map of active sessions. The server must be loaded with a `Plugin`to define incoming
/// and outgoing message handling.
pub struct Server<Q, P>
where
    Q: ServerResponse,
    P: ServerPlugin<Q>,
{
    me: Weak<RwLock<Self>>,
    sessions: HashMap<SessionId, Arc<Session>>,
    session_id_counter: AtomicUsize,
    plugin: P,
}

const BUFFER_SIZE: usize = 1024;

impl<Q, P> Server<Q, P>
where
    Q: ServerResponse + Send + 'static,
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
            })
        })
    }

    pub fn start_server(&self) {
        let my_clone = self.me.upgrade().unwrap();
        tokio::spawn(async move {
            let addr = SocketAddr::from(([127, 0, 0, 1], 3000));
            tracing::info!("Listening on {}", addr);
            println!("Listening on {}", addr);

            let app = Router::new()
                .route("/ws", get(ws_handler))
                .with_state(my_clone);

            if let Err(e) = axum_server::bind(addr).serve(app.into_make_service()).await {
                tracing::error!("Server failed to start: {}", e);
            }
        });
    }

    /// create_session
    ///
    /// Creates a new client session with a unique session ID. Returns a tuple containing
    /// the session handle, a receiver for responses, and the session ID.
    pub fn create_session(
        &mut self,
    ) -> Result<(UnboundedReceiver<String>, SessionId)> {
        let session_id = SessionId(format!(
            "session_{}",
            self.session_id_counter.fetch_add(1, Ordering::SeqCst)
        ));
        match self.sessions.entry(session_id.clone()) {
            Entry::Occupied(_) => Err(eyre!("Oh, no!")),
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
                eprintln!("Failed to destroy session: {}", e);
            }
        }
    }

    pub fn receive_message(
        &self,
        message: String,
        session_id: &SessionId,
    ) -> Result<(), Report> {
        self.plugin.process_incoming(message, session_id)
    }

    /// send_response
    ///
    /// Sends a response to the appropriate client session based on the session ID in the response.
    /// Returns a `Result` indicating success or failure if the session is not found.
    pub fn send_response(&self, response: Q) -> Result<()> {
        let session_id = response.get_session_id().clone();
        let processed_response = self.process_outgoing_message(response);
        match self.sessions.get(&session_id) {
            Some(session) => session.send_response(processed_response?),
            None => Err(eyre!("Oh, no!")),
        }
    }

    pub fn process_outgoing_message(&self, response: Q) -> Result<String, Report> {
        self.plugin.process_outgoing(response)
    }
}

async fn ws_handler<Q, P>(
    ws: WebSocketUpgrade,
    State(server): State<Arc<RwLock<Server<Q, P>>>>,
) -> impl IntoResponse
where
    Q: ServerResponse + Send + 'static,
    P: ServerPlugin<Q> + Send + Sync + 'static,
{
    ws.on_upgrade(move |mut ws: WebSocket| async move {
        let (mut receiver, session_id) = server.write().await.create_session().unwrap();
        loop {
            select! {
                // quit => { break; }
                res = ws.recv() => {
                    match res {
                        Some(Ok(message)) => {
                            if let axum::extract::ws::Message::Text(text) = message {
                                tracing::debug!("Received message: {}", text);
                                if let Err(e) = server.read().await.receive_message(text.to_string(), &session_id) {
                                    tracing::error!("Failed to process incoming message: {}", e);
                                    // Send error message back to the client


                                    let error_msg = format!("Error processing message: {}", e);
                                    let msg = server.read().await.plugin.process_error(error_msg, &session_id).unwrap(); 
                                    if let Err(e) = ws.send(Message::Text(msg)).await {
                                        tracing::error!("Failed to send WebSocket message: {}", e);

                                        
                                    break;
                            }
                                }
                            }
                        }
                        Some(Err(e)) => {
                            tracing::error!("WebSocket error: {}", e);
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
                            tracing::error!("WebSocket error: {}", e);
                            // Close the session?
                        }
                    }
                }
            }
        }
        server.write().await.close_session(&session_id);
    })
}
