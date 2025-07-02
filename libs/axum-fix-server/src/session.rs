use std::sync::Arc;

use axum::extract::ws::{Message, WebSocket};
use eyre::{eyre, Result};
use parking_lot::RwLock;
use tokio::{
    select,
    sync::mpsc::{UnboundedReceiver, UnboundedSender},
};
use tokio_util::sync::CancellationToken;

use crate::{messages::SessionId, server_plugin::ServerPlugin, server_state::ServerState};

/// Session
///
/// Manages a single client session for sending responses. Holds a sender
/// channel for responses and a unique session identifier.
pub struct Session {
    session_id: SessionId,
    response_tx: UnboundedSender<String>,
    cancel_token: CancellationToken,
}

impl Session {
    pub fn new(session_id: SessionId, tx: UnboundedSender<String>) -> Self {
        Self {
            session_id,
            response_tx: tx,
            cancel_token: CancellationToken::new(),
        }
    }

    pub fn get_session_id(&self) -> &SessionId {
        &self.session_id
    }

    pub fn cancel_token_cloned(&self) -> CancellationToken {
        self.cancel_token.clone()
    }

    pub fn send_response(&self, response: String) -> Result<()> {
        self.response_tx
            .send(response)
            .map_err(|err| eyre!("Error {:?}", err))
    }

    pub fn will_stop(&self) {
        self.cancel_token.cancel();
    }

    pub async fn wait_stopped(&self) -> Result<()> {
        // TODO: add some synchronisation for this
        Ok(())
    }

    pub async fn run_session(
        session_id: &SessionId,
        cancel_token: &CancellationToken,
        ws: &mut WebSocket,
        receiver: &mut UnboundedReceiver<String>,
    ) -> Result<Option<String>> {
        loop {
            select! {
                _ = cancel_token.cancelled() => {
                    break
                },
                res = ws.recv() => {
                    match res {
                        Some(Ok(message)) => {
                            if let Message::Text(text) = message {
                                return Ok(Some(text.to_string()));
                            }
                        }
                        Some(Err(err)) => {
                            return Err(eyre!("WebSocket error: {:?}", err));
                        }
                        None => {
                            tracing::info!("WebSocket connection closed on session {}", session_id);
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

        Ok(None)
    }
}
