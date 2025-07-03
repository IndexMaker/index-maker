use std::{
    sync::{Arc, Weak},
    thread::{self, spawn, JoinHandle},
    time::Duration,
};

use axum_fix_server::server::Server;
use crossbeam::channel::{select, Receiver};

use crate::{
    example_plugin::ExamplePlugin,
    fix_messages::{Body, FixHeader, FixTrailer},
    requests::Request,
    responses::Response,
};

#[derive(Debug)]
pub enum SolverEvents {
    Message(Request),
    Quit,
}

pub struct Solver {
    fix_server: Arc<Server<Response, ExamplePlugin<Request, Response>>>,
    event_rx: Receiver<SolverEvents>,
    handle: Option<JoinHandle<()>>,
}

impl Solver {
    pub fn new(
        fix_server: Arc<Server<Response, ExamplePlugin<Request, Response>>>,
        event_rx: Receiver<SolverEvents>,
    ) -> Self {
        Solver {
            fix_server,
            event_rx,
            handle: None,
        }
    }

    pub fn start(&mut self) {
        let event_rx = self.event_rx.clone();
        let fix_server = self.fix_server.clone();
        self.handle = Some(spawn(move || loop {
            select!(
                recv(event_rx) -> res => {
                    match res.unwrap() {
                        SolverEvents::Message(req) => {
                            // Reimplement handle_event logic here without referencing self
                            thread::sleep(Duration::from_secs(5));
                            let response = Response {
                                session_id:req.session_id.clone(),
                                standard_header:FixHeader{
                                    MsgType:"ACK".to_string(),
                                    SenderCompID:"server".to_string(),
                                    TargetCompID:req.standard_header.SenderCompID.clone(),
                                    SeqNum:1,
                                },
                                chain_id: req.chain_id,
                                address: req.address,
                                body:Body::ACKBody{
                                    RefSeqNum:req.standard_header.SeqNum,
                                },
                                standard_trailer:FixTrailer{
                                    PublicKey:vec!["serverKey".to_string()],
                                    Signature:vec!["serverSign".to_string()],
                                },
                            };

                            if let Err(e) = fix_server.send_response(response) {
                                tracing::error!("Failed to send response: {}", e);
                            }
                        }
                        SolverEvents::Quit => {
                            tracing::info!("Received Quit signal, terminating event loop thread.");
                            break;
                        }
                    }
                },
            )
        }));
    }

    pub fn stop(&mut self) {
        tracing::info!("Sent Quit signal to event loop thread.");
        if let Some(handle) = self.handle.take() {
            handle.join().unwrap();
        }
    }
}
