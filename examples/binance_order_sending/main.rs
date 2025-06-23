use core::time;
use std::{env, sync::Arc, thread::spawn, time::Duration};

use binance_order_sending::{binance_order_sending::BinanceOrderSending, credentials::Credentials};
use chrono::Utc;
use clap::Parser;
use crossbeam::{
    channel::{bounded, unbounded},
    select,
};
use parking_lot::RwLock;
use rust_decimal::dec;
use symm_core::{
    core::{
        bits::{Amount, BatchOrderId, OrderId, Side, SingleOrder, Symbol},
        functional::IntoObservableSingleArc,
        logging::log_init,
    },
    init_log,
    order_sender::order_connector::{OrderConnector, OrderConnectorNotification},
};
use tokio::{sync::mpsc::unbounded_channel, time::sleep};

#[derive(Parser)]
struct Cli {
    symbol: Symbol,
    side: Side,
    quantity: Amount,
    price: Amount,
}

#[tokio::main]
pub async fn main() {
    init_log!();

    let cli = Cli::parse();

    let api_key = env::var("BINANCE_API_KEY").expect("No API key in env");
    let credentials = Credentials::new(
        api_key,
        move || env::var("BINANCE_API_SECRET").ok(),
        move || env::var("BINANCE_PRIVATE_KEY_FILE").ok(),
        move || env::var("BINANCE_PRIVATE_KEY_PHRASE").ok(),
    );

    let (event_tx, event_rx) = unbounded::<OrderConnectorNotification>();
    let (end_tx, end_rx) = bounded::<()>(1);

    let order_sender = Arc::new(RwLock::new(BinanceOrderSending::new()));

    order_sender
        .write()
        .get_single_observer_arc()
        .write()
        .set_observer_from(event_tx);

    let (sess_tx, mut sess_rx) = unbounded_channel();

    let handle_event_internal = move |e: OrderConnectorNotification| match e {
        OrderConnectorNotification::SessionLogon {
            session_id,
            timestamp,
        } => {
            tracing::info!("Session Logon {} at {}", session_id, timestamp);
            sess_tx
                .send(Some(session_id))
                .expect("Failed to notify session logon");
        }
        OrderConnectorNotification::SessionLogout {
            session_id,
            reason,
            timestamp,
        } => {
            tracing::info!(
                "Session Logout {} at {} - {}",
                session_id,
                timestamp,
                reason
            );
            sess_tx.send(None).expect("Failed to notify session logout");
        }
        OrderConnectorNotification::Rejected {
            order_id,
            symbol,
            side,
            price,
            quantity,
            reason,
            timestamp,
        } => {
            tracing::warn!(
                "Rejected {} {} {:?} {} {} {}: {}",
                order_id,
                symbol,
                side,
                price,
                quantity,
                timestamp,
                reason,
            );
        }
        OrderConnectorNotification::Fill {
            order_id,
            lot_id,
            symbol,
            side,
            price,
            quantity,
            fee,
            timestamp,
        } => {
            tracing::info!(
                "Fill {} {} {} {:?} {} {} {} {}",
                order_id,
                lot_id,
                symbol,
                side,
                price,
                quantity,
                fee,
                timestamp
            );
        }
        OrderConnectorNotification::Cancel {
            order_id,
            symbol,
            side,
            quantity,
            timestamp,
        } => {
            tracing::info!(
                "Cancel {} {} {:?} {} {}",
                order_id,
                symbol,
                side,
                quantity,
                timestamp
            );
        }
    };

    let task = spawn(move || {
        tracing::info!("Loop started");
        loop {
            select! {
                recv(event_rx) -> res => {
                    handle_event_internal(res.unwrap());
                },
                recv(end_rx) -> _ => {
                    break;
                }
            }
        }
        tracing::info!("Loop exited");
    });

    order_sender
        .write()
        .start()
        .expect("Failed to start order sender");

    order_sender
        .write()
        .logon(Some(credentials))
        .expect("Failed to logon");

    // wait for logon
    let session_id = sess_rx
        .recv()
        .await
        .expect("Failed to await logon")
        .as_ref()
        .cloned()
        .expect("Session not logged on");

    let timestamp = Utc::now();

    order_sender
        .write()
        .send_order(
            session_id,
            &Arc::new(SingleOrder {
                order_id: OrderId::from(format!("O-{}", timestamp.timestamp_millis())),
                batch_order_id: BatchOrderId::from(format!("B-{}", timestamp.timestamp_millis())),
                symbol: cli.symbol,
                side: cli.side,
                price: cli.price,
                quantity: cli.quantity,
                created_timestamp: timestamp,
            }),
        )
        .expect("Failed to send order");

    order_sender
        .write()
        .stop()
        .await
        .expect("Failed to stop order sender");

    sleep(Duration::from_secs(10)).await;

    end_tx.send(()).expect("Failed to send stop");
    task.join().expect("Failed to await task");
}
