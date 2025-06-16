use std::{
    env,
    sync::{atomic::AtomicPtr, Arc},
    thread::spawn,
    time::Duration,
};

use binance_order_sending::binance_order_sending::BinanceOrderSending;
use binance_spot_connector_rust::http::Credentials;
use chrono::Utc;
use crossbeam::{channel::unbounded, select};
use index_maker::{
    core::{
        bits::{Side, SingleOrder},
        functional::IntoObservableSingleArc,
    },
    order_sender::order_connector::{OrderConnector, OrderConnectorNotification, SessionId},
};
use parking_lot::RwLock;
use rust_decimal::dec;
use tokio::{sync::watch::channel, time::sleep};

#[tokio::main]
pub async fn main() {
    let api_key = env::var("MY_BINANCE_API_KEY").expect("No API key in env");
    let api_secret = env::var("MY_BINANCE_API_SECRET").expect("No API secret in env");
    let credentials = Credentials::from_hmac(api_key, api_secret);

    let (event_tx, event_rx) = unbounded::<OrderConnectorNotification>();

    let order_sender = Arc::new(RwLock::new(BinanceOrderSending::new()));

    order_sender
        .write()
        .get_single_observer_arc()
        .write()
        .set_observer_from(event_tx);

    let (sess_tx, mut sess_rx) = channel(None);

    let handle_event_internal = move |e: OrderConnectorNotification| match e {
        OrderConnectorNotification::SessionLogon { session_id } => {
            println!("(binance-order-sender-main) Session Logon {}", session_id);
            sess_tx
                .send(Some(session_id))
                .expect("Failed to notify session logon");
        }
        OrderConnectorNotification::SessionLogout { session_id } => {
            println!("(binance-order-sender-main) Session Logout {}", session_id);
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
            println!(
                "(binance-order-sender-main) Fill {} {} {} {:?} {} {} {} {}",
                order_id, lot_id, symbol, side, price, quantity, fee, timestamp
            );
        }
        OrderConnectorNotification::Cancel {
            order_id,
            symbol,
            side,
            quantity,
            timestamp,
        } => {
            println!(
                "(binance-order-sender-main) Cancel {} {} {:?} {} {}",
                order_id, symbol, side, quantity, timestamp
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
        .wait_for(|v| v.is_some())
        .await
        .expect("Failed to await logon")
        .as_ref()
        .cloned()
        .unwrap();

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
