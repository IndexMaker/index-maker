use std::time::Duration;

use axum::extract::ws::{Message, WebSocket};
use eyre::{eyre, Result};
use futures_util::pin_mut;
use tokio::{
    select,
    sync::mpsc::{UnboundedReceiver, UnboundedSender},
    time::{interval_at, sleep, Instant, MissedTickBehavior},
};
use tokio_util::sync::CancellationToken;

use crate::messages::SessionId;

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

    pub async fn run_session(
        session_id: &SessionId,
        cancel_token: &CancellationToken,
        ws: &mut WebSocket,
        receiver: &mut UnboundedReceiver<String>,
    ) -> Result<Option<String>> {
        let mut ping_interval = interval_at(Instant::now() + Duration::from_secs(30), Duration::from_secs(30));
        ping_interval.set_missed_tick_behavior(MissedTickBehavior::Skip);
        
        let mut waiting_pong = false;
        let pong_timeout = Duration::from_secs(10);
        let inactivity_timeout = sleep(pong_timeout);
        pin_mut!(inactivity_timeout);
        
        loop {
            select! {
                _ = cancel_token.cancelled() => {
            tracing::info!("Session {} cancelled", session_id);
                    break;
                },
                _ = ping_interval.tick() => {
                    if let Err(err) = ws.send(Message::Ping(vec![])).await {
                        tracing::warn!("Failed to send ping to session {}: {}", session_id, err);
                        break;
                    }
                    tracing::debug!("Sent ping to session {}", session_id);
                    // Reset inactivity timeout after sending ping
                    waiting_pong = true;
                    inactivity_timeout.as_mut().reset(Instant::now() + pong_timeout);
                },
                _ = &mut inactivity_timeout => {
                    if waiting_pong {
                        tracing::warn!("No activity from session {} within timeout, closing", session_id);
                        break;
                    }
                },
                res = ws.recv() => {
                    match res {
                        Some(Ok(message)) => {
                            if let Message::Text(text) = message {
                                return Ok(Some(text.to_string()));
                            }
                            if let Message::Pong(_) = message {
                                tracing::debug!("Received pong from session {}", session_id);
                            }
                            waiting_pong = false;
                            ping_interval.reset();
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
