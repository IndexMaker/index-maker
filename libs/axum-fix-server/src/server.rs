use std::sync::Arc;

use crate::{server_plugin::ServerPlugin, server_state::ServerState, session::Session};
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
use futures_util::future::join_all;
use itertools::Itertools;
use parking_lot::RwLock;
use std::net::SocketAddr;

/// Server
///
/// The core server structure that manages client sessions and handles incoming requests,
/// it maintains a map of all active sessions and handles their lifetime. The server
/// must be loaded with a `Plugin` to define incoming and outgoing message handling.
/// The `Plugin` consumes the message and is responsible for deserialization,
/// message validation (fields, seqnum, signatures), publishing to application, etc.
pub struct Server<Response, Plugin> {
    server_state: Arc<RwLock<ServerState<Response, Plugin>>>,
}

impl<Response, Plugin> Server<Response, Plugin>
where
    Response: Send + Sync + 'static,
    Plugin: ServerPlugin<Response> + Send + Sync + 'static,
{
    /// new
    ///
    /// Creates a new instance of `Server`, an empty map of sessions, and a new observer.
    /// Initializes the session ID counter to start from 1.
    pub fn new(plugin: Plugin) -> Self {
        Self {
            server_state: Arc::new(RwLock::new(ServerState::new(plugin))),
        }
    }

    pub fn with_plugin<Ret>(&self, cb: impl FnOnce(&Plugin) -> Ret) -> Ret {
        let state = self.server_state.read();
        cb(state.get_plugin())
    }

    pub fn with_plugin_mut<Ret>(&self, cb: impl FnOnce(&mut Plugin) -> Ret) -> Ret {
        let mut state = self.server_state.write();
        cb(state.get_plugin_mut())
    }

    /// start_server
    ///
    /// Initializes the server, spawining it on a thread using `ws_handler` logic.
    pub fn start_server(&self, address: String) -> Result<()> {
        let server_state = self.server_state.clone();

        tokio::spawn(async move {
            let addr: SocketAddr = address.parse().expect(&format!(
                "Server failed to start: Invalid address ({})",
                address
            ));
            tracing::info!("Listening on {}", addr);

            let app = Router::new()
                .route("/ws", get(ws_handler))
                .with_state(server_state);

            if let Err(e) = axum_server::bind(addr).serve(app.into_make_service()).await {
                tracing::warn!("Server failed to start: {}", e);
            }
        });

        Ok(())
    }

    /// close_server
    ///
    /// Closes server for new connections
    pub fn close_server(&self) {
        self.server_state.write().close();
    }

    /// close_server
    ///
    /// Closes all sessions
    pub async fn stop_server(&self) -> Result<()> {
        let sessions = self.server_state.write().close_all_sessions()?;
        let stop_futures = sessions.iter().map(|s| s.wait_stopped()).collect_vec();

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

    /// send_response
    ///
    /// Sends a response to the appropriate client session based on the session ID in the response.
    /// Returns a `Result` indicating success or failure if the session is not found.
    pub fn send_response(&self, response: Response) -> Result<()> {
        self.server_state.write().process_outgoing(response)
    }
}

/// ws_handler
///
/// This is the closure that contains the async logic used by the `Server`. On a WS upgrade it
/// creates a new session and awaits for either a message from the client (on the ws), or
/// for a response coming from the server (on the tokio channel). In case of an error, or the
/// session is closed by the client, the loop is broken and the session destroyed.
async fn ws_handler<Response, Plugin>(
    ws: WebSocketUpgrade,
    State(server_state): State<Arc<RwLock<ServerState<Response, Plugin>>>>,
) -> impl IntoResponse
where
    Response: Send + Sync + 'static,
    Plugin: ServerPlugin<Response> + Send + Sync + 'static,
{
    ws.on_upgrade(async move |mut ws: WebSocket| {
        if !server_state.read().is_accepting_connections() {
            return;
        }

        let (mut rx, session) = match server_state.write().create_session() {
            Err(err) => {
                tracing::warn!("Failed to create session: {:?}", err);
                return;
            }
            Ok(x) => x,
        };

        let session_id = session.get_session_id().clone();
        let cancel_token = session.cancel_token_cloned();

        loop {
            let result =
                match Session::run_session(&session_id, &cancel_token, &mut ws, &mut rx).await {
                    Ok(res) => res,
                    Err(err) => {
                        tracing::warn!("An error encountered while running session: {:?}", err);
                        break;
                    }
                };

            // Note: We split the loop so that read-lock is acquired outside of
            // any await

            let incoming_message = match result {
                Some(incoming_message) => incoming_message,
                None => {
                    break;
                }
            };

            let result = server_state
                .read()
                .process_incoming(incoming_message, &session_id);

            match result {
                Ok(result) => {
                    if let Err(err) = ws.send(Message::Text(result.to_string())).await {
                        tracing::warn!("Failed to send WebSocket message: {}", err);
                        break;
                    }
                }
                Err(err) => {
                    tracing::warn!("Failed to process incoming message: {}", err);

                    if let Err(err) = ws.send(Message::Text(err.to_string())).await {
                        tracing::warn!("Failed to send WebSocket message: {}", err);
                        break;
                    }
                }
            }
        }

        if let Err(err) = server_state.write().close_session(session_id) {
            tracing::warn!("Failed to close session: {:?}", err);
        }
    })
}
