use std::{env, sync::Arc, thread::spawn, time::Duration};

use binance_order_sending::{binance_order_sending::BinanceOrderSending, credentials::Credentials};
use chrono::Utc;
use crossbeam::{
    channel::{self, bounded, unbounded},
    select,
};
use index_maker::{
    core::{
        bits::{BatchOrderId, OrderId, Side, SingleOrder, Symbol},
        functional::IntoObservableSingleArc,
        logging::log_init,
    },
    init_log,
    order_sender::order_connector::{OrderConnector, OrderConnectorNotification},
};
use parking_lot::RwLock;
use rust_decimal::dec;
use tokio::{sync::mpsc::unbounded_channel, time::sleep};

#[tokio::main]
pub async fn main() {
    init_log!();

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
        OrderConnectorNotification::SessionLogon { session_id } => {
            tracing::info!("Session Logon {}", session_id);
            sess_tx
                .send(Some(session_id))
                .expect("Failed to notify session logon");
        }
        OrderConnectorNotification::SessionLogout { session_id, reason } => {
            tracing::info!("Session Logout {} - {}", session_id, reason);
            sess_tx.send(None).expect("Failed to notify session logout");
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

    order_sender
        .write()
        .send_order(
            session_id,
            &Arc::new(SingleOrder {
                order_id: OrderId::from(format!("O-{}", Utc::now().timestamp_millis())),
                batch_order_id: BatchOrderId::from("B-1"),
                symbol: Symbol::from("BNBEUR"),
                side: Side::Sell,
                price: dec!(559.60),
                quantity: dec!(0.02),
                created_timestamp: Utc::now(),
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
