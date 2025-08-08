use axum::extract::ws::{Message, WebSocket};
use chrono::Utc;
use eyre::eyre;
use itertools::Itertools;
use tokio::{select, sync::mpsc::UnboundedReceiver, time::sleep};
use tokio_util::sync::CancellationToken;

use crate::messages::SessionId;

pub enum RunSessionResult {
    MessageReceived(String),
    Error(eyre::Report),
    ConnectionClosed,
    Continue,
}

pub struct SessionState<'a> {
    session_id: SessionId,
    ws: &'a mut WebSocket,
    receiver: &'a mut UnboundedReceiver<String>,
    cancel_token: CancellationToken,
    timeout: std::time::Duration,
    ping_timestamp: Option<i64>,
}

impl<'a> SessionState<'a> {
    pub fn new(
        session_id: SessionId,
        ws: &'a mut WebSocket,
        receiver: &'a mut UnboundedReceiver<String>,
        cancel_token: CancellationToken,
        timeout: std::time::Duration,
    ) -> Self {
        Self {
            session_id,
            ws,
            receiver,
            cancel_token,
            timeout,
            ping_timestamp: None,
        }
    }

    pub async fn send_message(&mut self, message: String) -> eyre::Result<()> {
        self.ws
            .send(Message::Text(message))
            .await
            .map_err(|err| eyre!("Failed to send message {:?}", err))
    }

    pub async fn send_error(&mut self, report: eyre::Report) -> eyre::Result<()> {
        self.ws
            .send(Message::Text(report.to_string())) //< TODO: We shouldn't use Report for NAKs
            .await
            .map_err(|err| eyre!("Failed to send message {:?}", err))
    }

    async fn send_ping(&mut self) -> eyre::Result<i64> {
        let ping_timestamp = Utc::now().timestamp_millis();
        let ping_data = ping_timestamp.to_le_bytes().into_iter().collect_vec();

        if let Err(err) = self.ws.send(Message::Ping(ping_data)).await {
            Err(eyre!("Failed to send ping: {:?}", err))?
        }

        Ok(ping_timestamp)
    }

    fn parse_pong_data(&self, pong_data: Vec<u8>) -> eyre::Result<i64> {
        let pong_timestamp_bytes: [u8; 8] = pong_data
            .try_into()
            .map_err(|err| eyre!("Failed to read pong data: {:?}", err))?;

        let pong_timestamp = i64::from_le_bytes(pong_timestamp_bytes);

        Ok(pong_timestamp)
    }

    fn process_pong_data(&mut self, pong_data: Vec<u8>) -> eyre::Result<()> {
        match (self.ping_timestamp, self.parse_pong_data(pong_data)) {
            (Some(a), Ok(b)) => {
                if a == b {
                    self.ping_timestamp = None;
                    Ok(())
                } else {
                    Err(eyre!("Invalid pong data received"))
                }
            }
            (_, Err(err)) => Err(eyre!("Failed to parse pong data: {:?}", err)),
            (None, _) => Err(eyre!("Unexpected pong received")),
        }
    }

    fn process_message(&mut self, message: Message) -> RunSessionResult {
        match message {
            Message::Text(text) => RunSessionResult::MessageReceived(text),
            Message::Pong(pong_data) => {
                if let Err(err) = self.process_pong_data(pong_data) {
                    RunSessionResult::Error(err)
                } else {
                    RunSessionResult::Continue
                }
            }
            Message::Binary(_) => {
                // We could support binary FIX protocol here
                RunSessionResult::Error(eyre!("Unexpected binary received"))
            }
            Message::Ping(_) => {
                // Tungstenite should have handled that
                RunSessionResult::Error(eyre!("Unexpected ping received"))
            }
            Message::Close(_) => RunSessionResult::ConnectionClosed,
        }
    }

    async fn process_timeout(&mut self) -> RunSessionResult {
        if self.ping_timestamp.is_some() {
            return RunSessionResult::Error(eyre!("Connection timeout"));
        }

        match self.send_ping().await {
            Ok(res) => {
                tracing::info!(
                    session_id = %self.session_id,
                    ping_timestamp = %res,
                    "Ping sent");

                self.ping_timestamp = Some(res);
                RunSessionResult::Continue
            }
            Err(err) => RunSessionResult::Error(eyre!("Failed to send ping: {:?}", err)),
        }
    }

    pub async fn get_message(&mut self) -> RunSessionResult {
        loop {
            select! {
                _ = self.cancel_token.cancelled() => {
                    tracing::info!(
                        session_id = %self.session_id,
                        "Session terminated");

                    return RunSessionResult::ConnectionClosed;
                },
                res = self.ws.recv() => {
                    match res {
                        Some(Ok(message)) => {
                            return self.process_message(message);
                        }
                        Some(Err(err)) => {
                            return RunSessionResult::Error(
                                eyre!("WebSocket error: {:?}", err));
                        }
                        None => {
                            return RunSessionResult::ConnectionClosed;
                        }
                    }
                }
                Some(res) = self.receiver.recv() => {
                    match self.ws.send(Message::Text(res)).await {
                        Ok(()) => {},
                        Err(err) => {
                            return RunSessionResult::Error(
                                eyre!("WebSocket error: {:?}", err)
                            );
                        }
                    }
                }
                _ = sleep(self.timeout) => {
                    return self.process_timeout().await;
                }
            }
        }
    }
}
