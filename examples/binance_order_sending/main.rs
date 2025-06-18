use std::{env, sync::Arc, thread::spawn, time::Duration};

use binance_order_sending::{binance_order_sending::BinanceOrderSending, session::Credentials};
use chrono::Utc;
use crossbeam::{channel::unbounded, select};
use index_maker::{
    core::{
        bits::{Side, SingleOrder},
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

    let api_key = env::var("MY_BINANCE_API_KEY").expect("No API key in env");
    let credentials = Credentials::new(api_key, move || {
        env::var("MY_BINANCE_API_SECRET").expect("No API secret in env")
    });

    let (event_tx, event_rx) = unbounded::<OrderConnectorNotification>();

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

    spawn(move || loop {
        select! {
            recv(event_rx) -> res => {
                handle_event_internal(res.unwrap());
            }
        }
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
                order_id: "O1".into(),
                batch_order_id: "B1".into(),
                symbol: "A1".into(),
                side: Side::Buy,
                price: dec!(1.0),
                quantity: dec!(1.0),
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
}
