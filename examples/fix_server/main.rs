use std::{
    sync::Arc,
    thread::{sleep, spawn},
    time::Duration,
};

use axum_fix_server::{plugins::server_plugin::SerdePlugin, server::Server};
use crossbeam::{channel::unbounded, select};
use index_maker::{core::logging::log_init, init_log};
mod fix_messages;
mod requests;
mod responses;

use requests::Request;
use responses::Response;

use crate::fix_messages::{ACKBody, FixHeader, FixTrailer};

fn handle_server_event(event: &Request) {
    // println!(
    //     "handle_server_event >> {} {} {}",
    //     event.session_id, event.address, event.quantity
    // );
}

#[tokio::main]
pub async fn main() {
    init_log!();
    let plugin = SerdePlugin::<Request, Response>::new();
    let fix_server = Server::new_arc(plugin);
    //let plugin = DummyPlugin;
    //let fix_server = Arc::new(RwLock::new(Server::<MyServerRequest, MyServerResponse, DummyPlugin>::new(plugin)));

    let (event_tx, event_rx) = unbounded::<Request>();

    fix_server
        .write()
        .await
        .get_multi_observer_mut()
        .set_observer_fn(move |e: Request| {
            handle_server_event(&e);
            event_tx.send(e).unwrap();
        });

    let fix_server_weak = Arc::downgrade(&fix_server);

    let handle_server_event_internal = move |e: Request| {
        let fix_server = fix_server_weak.upgrade().unwrap();
        let response = match e {
            Request::NewOrderSingle {session_id, StandardHeader, Body, StandardTrailer } =>  {
                println!("deserialize_from_fix: Session ID set to {}", session_id);
                Response::ACK {
                    session_id: session_id,
                    StandardHeader: FixHeader {
                        MsgType: "ACK".to_string(),
                        SenderCompID: "server".to_string(),
                        TargetCompID: StandardHeader.SenderCompID,
                        SeqNum: 1,
                    },
                    Body: ACKBody{
                        RefSeqNum: StandardHeader.SeqNum,
                    },
                    StandardTrailer: FixTrailer{
                        PublicKey: vec!["serverKey".to_string()],
                        Signature: vec!["serverSign".to_string()],
                    },
                }
            }
            
        };
        
        fix_server.blocking_read().send_response(response);
    };

    spawn(move || loop {
        select!(
            recv(event_rx) -> res => {
                handle_server_event_internal(res.unwrap())
            },
        )
    });

    fix_server.read().await.start_server(); //< should launch async task, and return immediatelly

    sleep(Duration::from_secs(600));
}
