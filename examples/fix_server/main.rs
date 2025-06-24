use std::{
    sync::Arc,
    thread::{sleep, spawn},
    time::Duration,
};

use axum_fix_server::server::Server;
use crossbeam::{channel::unbounded, select};
use symm_core::{core::logging::log_init, init_log};
mod example_plugin;
mod fix_messages;
mod requests;
mod responses;

use example_plugin::CompositeServerPlugin;
use requests::Request;
use responses::Response;

use crate::fix_messages::{Body, FixHeader, FixTrailer};

#[derive(Debug)]
enum ServerEvent {
    Message(Request),
    Quit,
}


#[tokio::main]
pub async fn main() {
    init_log!();
    let plugin = CompositeServerPlugin::<Request, Response>::new();
    let fix_server = Server::new_arc(plugin);
 
    let (event_tx, event_rx) = unbounded::<ServerEvent>();
    let event_tx_clone = event_tx.clone();

    fix_server
        .write()
        .await
        .get_multi_observer_mut()
        .set_observer_fn(move |e: Request| {
            //handle_server_event(&e);
            event_tx.send(ServerEvent::Message(e)).unwrap();
        });

    let fix_server_weak = Arc::downgrade(&fix_server);

    let handle_server_event_internal = move |e: Request| {
        sleep(Duration::from_secs(5));
        let fix_server = fix_server_weak.upgrade().unwrap();
        let response = Response {
            session_id: e.session_id.clone(),
            standard_header: FixHeader {
                MsgType: "ACK".to_string(),
                SenderCompID: "server".to_string(),
                TargetCompID: e.standard_header.SenderCompID,
                SeqNum: 1,
            },
            body: Body::ACKBody {
                RefSeqNum: e.standard_header.SeqNum,
            },
            standard_trailer: FixTrailer {
                PublicKey: vec!["serverKey".to_string()],
                Signature: vec!["serverSign".to_string()],
            },
        };

        fix_server.blocking_read().send_response(response);
    };

    // Capture the JoinHandle to wait for the thread later
    let handle = spawn(move || loop {
        select!(
            recv(event_rx) -> res => {
                match res.unwrap() {
                    ServerEvent::Message(req) => handle_server_event_internal(req),
                    ServerEvent::Quit => {
                        println!("Received Quit signal, terminating event loop thread.");
                        break;
                    }
                }
            },
        )
    });
    fix_server.read().await.start_server(); //< should launch async task, and return immediatelly

    sleep(Duration::from_secs(600));
    event_tx_clone.send(ServerEvent::Quit).unwrap();
    println!("Sent Quit signal to event loop thread.");

    // Wait for the spawned thread to finish using join
    handle.join().unwrap();
    println!("Event loop thread has terminated, main thread exiting.");
}
