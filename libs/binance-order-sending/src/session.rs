use std::sync::Arc;

use async_core::async_loop::AsyncLoop;
use binance_spot_connector_rust::{http::Credentials, tokio_tungstenite::BinanceWebSocketClient};
use chrono::Utc;
use eyre::{eyre, Result};
use futures_util::StreamExt;
use index_maker::{
    core::{
        bits::Side,
        functional::{PublishSingle, SingleObserver},
    },
    order_sender::order_connector::OrderConnectorNotification,
};
use parking_lot::RwLock as AtomicLock;
use rust_decimal::dec;
use tokio::{
    select,
    sync::mpsc::{UnboundedReceiver, UnboundedSender},
};

use crate::command::Command;

pub struct Session {
    command_tx: UnboundedSender<Command>,
    session_loop: AsyncLoop<Credentials>,
}

impl Session {
    pub fn new(command_tx: UnboundedSender<Command>) -> Self {
        Self {
            command_tx,
            session_loop: AsyncLoop::new(),
        }
    }

    pub fn send_command(&self, command: Command) -> Result<()> {
        self.command_tx
            .send(command)
            .map_err(|err| eyre!("Failed to send command to session {}", err))
    }

    pub fn start(
        &mut self,
        mut command_rx: UnboundedReceiver<Command>,
        observer: Arc<AtomicLock<SingleObserver<OrderConnectorNotification>>>,
        credentials: Credentials,
    ) {
        self.session_loop.start(async move |cancel_token| {
            let (mut conn, _) =
                BinanceWebSocketClient::connect_async("wss://ws-api.binance.com:443/ws-api/v3")
                    .await
                    .expect("failed to connect to Binance");
            println!("(binance-session) Logon");
            // conn.send(LOGON, credentials)
            observer
                .write()
                .publish_single(OrderConnectorNotification::SessionLogon {
                    session_id: credentials.api_key.as_str().into(),
                });
            loop {
                select! {
                    _ = cancel_token.cancelled() => {
                        break;
                    },
                    Some(command) = command_rx.recv() => {
                        match command {
                            Command::NewOrder(order) => {
                                println!("(binance-session) Command::NewOrder");
                                // conn.send(ORDER, ...)
                            },
                        }
                    },
                    Some(result) = conn.as_mut().next() => {
                        println!("(binance-session) OrderConnectorNotification::Fill");
                        // TODO parse message and publish
                        observer.read().publish_single(OrderConnectorNotification::Fill {
                            order_id: "1".into(),
                            lot_id: "L".into(),
                            symbol: "A".into(),
                            side: Side::Buy,
                            price: dec!(1.0),
                            quantity: dec!(1.0),
                            fee: dec!(1.0),
                            timestamp: Utc::now() });
                    }
                }
            }
            println!("(binance-session) Logout");
            observer
                .write()
                .publish_single(OrderConnectorNotification::SessionLogout {
                    session_id: credentials.api_key.as_str().into(),
                });
            credentials
        });
    }
}
