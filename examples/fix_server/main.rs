use std::{sync::Arc, thread::sleep, time::Duration};

use axum_fix_server::server::Server;
use crossbeam::channel::unbounded;
use symm_core::{core::logging::log_init, init_log};
mod example_plugin;
mod fix_messages;
mod requests;
mod responses;
mod solver;

use example_plugin::ExamplePlugin;
use requests::Request;
use responses::Response;
use solver::{Solver as MockSolver, SolverEvents};

#[tokio::main]
pub async fn main() {
    init_log!();
    let (event_tx, event_rx) = unbounded::<SolverEvents>();
    let event_tx_clone = event_tx.clone();

    // Creating plugin that will consume server messages
    let mut plugin = ExamplePlugin::<Request, Response>::new();
    plugin.set_observer_plugin_callback(Box::new(move |e: &Request| {
        event_tx.send(SolverEvents::Message(e.clone()));
    }));

    //Creating server
    let fix_server = Arc::new(Server::new(plugin));

    // Creating and starting a mock solver, to simulate some work being done and respond to messages
    let mut solver = MockSolver::new(fix_server.clone(), event_rx);
    solver.start();

    // Starting the server and wainting connections
    fix_server.start_server("127.0.0.1:3000".to_string());

    // Sleeping on main thread, server thread and solver thread are working
    sleep(Duration::from_secs(600));

    // Stops theserver from accepting new connections
    fix_server.close_server();

    // Send quit message to solver, for gracious shutdow
    // stop wil await for solver to stop processing
    println!("Sent Quit signal to event loop thread.");
    event_tx_clone.send(SolverEvents::Quit).unwrap();
    solver.stop();

    // Closes all server sessions
    fix_server
        .stop_server()
        .await
        .expect("Failed to stop server");
}
