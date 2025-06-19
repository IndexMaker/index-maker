use std::{
    collections::{hash_map::Entry, HashMap},
    future::Future,
    marker::PhantomData,
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
use index_maker::core::functional::{MultiObserver, PublishMany, PublishSingle, SingleObserver};
use itertools::Itertools;
use std::net::SocketAddr;
use tokio::{
    select,
    sync::{
        mpsc::{channel, unbounded_channel, UnboundedReceiver, UnboundedSender},
        RwLock,
    },
};

use crate::{
    messages::{FixMessage, FixMessageBuilder, ServerRequest, ServerResponse, SessionId},
    plugins::server_plugin::ServerPlugin,
};

/// Session
///
/// Manages a single client session for sending responses. Holds a sender channel for
/// responses and a unique session identifier.
pub(crate) struct Session<Q>
where
    Q: ServerResponse,
{
    response_tx: UnboundedSender<Q>,
    session_id: SessionId,
}

impl<Q> Session<Q>
where
    Q: ServerResponse,
{
    /// new
    ///
    /// Creates a new Session.
    pub fn new(tx: UnboundedSender<Q>, session_id: SessionId) -> Self {
        Self {
            response_tx: tx,
            session_id,
        }
    }

    /// send_response
    ///
    /// Sends a response to the client by enqueueing a message
    /// to the receivieng channel held by 'ws_handler'.
    /// Returns a `Result` indicating success or failure.
    pub fn send_response(&self, response: Q) -> Result<()> {
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
pub struct Server<R, Q, P>
where
    R: ServerRequest,
    Q: ServerResponse,
    P: ServerPlugin<R, Q>,
{
    me: Weak<RwLock<Self>>,
    observer: SingleObserver<R>, //Option<Box<dyn Fn(&R)>>,
    sessions: HashMap<SessionId, Arc<Session<Q>>>,
    session_id_counter: AtomicUsize,
    plugin: P,
}

const BUFFER_SIZE: usize = 1024;

impl<R, Q, P> Server<R, Q, P>
where
    R: ServerRequest + Send + 'static,
    Q: ServerResponse + Send + 'static,
    P: ServerPlugin<R, Q> + Send + Sync + 'static,
{
    /// new
    ///
    /// Creates a new instance of `Server`, an empty map of sessions, and a new observer.
    /// Initializes the session ID counter to start from 1.
    pub fn new_arc(plugin: P) -> Arc<RwLock<Self>> {
        Arc::new_cyclic(|me| {
            RwLock::new(Self {
                me: me.clone(),
                observer: SingleObserver::new(),
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

    /// send_response
    ///
    /// Sends a response to the appropriate client session based on the session ID in the response.
    /// Returns a `Result` indicating success or failure if the session is not found.
    pub fn send_response(&self, response: Q) -> Result<()> {
        let session_id = response.get_session_id().clone();
        match self.sessions.get(&session_id) {
            Some(session) => session.send_response(response),
            None => Err(eyre!("Oh, no!")),
        }
    }

    /// handle_server_message
    ///
    /// Processes an incoming server request by publishing it to all registered observers for processing.
    pub fn handle_server_message(&self, request: R) {
        self.observer.publish_single(request);
    }

    /// create_session
    ///
    /// Creates a new client session with a unique session ID. Returns a tuple containing
    /// the session handle, a receiver for responses, and the session ID.
    pub(crate) fn create_session(
        &mut self,
    ) -> Result<(Arc<Session<Q>>, UnboundedReceiver<Q>, SessionId)> {
        let session_id = SessionId(format!(
            "session_{}",
            self.session_id_counter.fetch_add(1, Ordering::SeqCst)
        ));
        match self.sessions.entry(session_id.clone()) {
            Entry::Occupied(_) => Err(eyre!("Oh, no!")),
            Entry::Vacant(vacant_entry) => {
                let (tx, rx) = unbounded_channel();
                let session =
                    vacant_entry.insert(Arc::new(Session::<Q>::new(tx, session_id.clone())));
                Ok((session.clone(), rx, session_id))
            }
        }
    }

    pub fn build_fix_message(&self, q: Q) -> Result<FixMessage> {
        let builder = FixMessageBuilder::new();
        let message = q.serialize_into_fix(builder);
        message
    }

    pub async fn process_incoming_message(
        &self,
        message: String,
        session_id: SessionId,
    ) -> Result<(), Report> {
        let request = self.plugin.process_incoming(message, session_id)?;
        self.handle_server_message(request);
        Ok(())
    }

    pub fn process_outgoing_message(&self, response: Q) -> Result<String, Report> {
        self.plugin.process_outgoing(&response)
    }

    pub fn get_multi_observer_mut(&mut self) -> &mut SingleObserver<R> {
        &mut self.observer
    }

    // Function to allow plugin trait implementation by composition
    // pub fn with_plugin<NewP>(&self, new_plugin: NewP) -> Server<R, Q, NewP>
    // where
    //     NewP: ServerPlugin<R, Q>,
    // {
    // todo!();
    // }
}

async fn ws_handler<R, Q, P>(
    ws: WebSocketUpgrade,
    State(server): State<Arc<RwLock<Server<R, Q, P>>>>,
) -> impl IntoResponse
where
    R: ServerRequest + Send + 'static,
    Q: ServerResponse + Send + 'static,
    P: ServerPlugin<R, Q> + Send + Sync + 'static,
{
    ws.on_upgrade(move |mut ws: WebSocket| async move {
        let (session, mut receiver, session_id) = server.write().await.create_session().unwrap();
        loop {
            select! {
                // quit => { break; }
                Some(res) = ws.recv() => {
                    match res {
                        Ok(message) => {
                            if let axum::extract::ws::Message::Text(text) = message {
                                tracing::debug!("Received message: {}", text);
                                if let Err(e) = server.read().await.process_incoming_message(text.to_string(), session_id.clone()).await {
                                    tracing::error!("Failed to process incoming message: {}", e);
                                }
                            }
                        }
                        Err(e) => {
                            tracing::error!("WebSocket error: {}", e);
                            break;
                        }
                    }
                }
                Some(res) = receiver.recv() => {
                    match server.write().await.build_fix_message(res) {
                        Ok(fix_message) => {
                            if let Err(e) = ws.send(Message::Text(fix_message.0.into())).await {
                                tracing::error!("Failed to send WebSocket message: {}", e);
                                break;
                            }
                        }
                        Err(e) => {
                            tracing::error!("Failed to build FIX message: {}", e);
                        }
                    }
                }
            }
        }
    })
}
