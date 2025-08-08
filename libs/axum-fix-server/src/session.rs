use std::sync::Arc;

use axum::extract::ws::{Message, WebSocket};
use chrono::Utc;
use eyre::{eyre, Result};
use futures_util::TryFutureExt;
use itertools::Itertools;
use tokio::{
    select,
    sync::mpsc::{UnboundedReceiver, UnboundedSender},
    time::sleep,
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
        tracing::info!("Session created: {}", session_id);
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
        if self.response_tx.is_closed() {
            tracing::warn!(
                target: "axum-fix-session",
                json_data = %response,
                message = "Cannot send to closed session");

            Err(eyre!("Session is closed: {}", self.session_id))?;
            self.will_stop();
        }
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
}
