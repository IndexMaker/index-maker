use std::{
    sync::Arc,
    thread::{sleep, spawn},
    time::Duration,
};

use alloy::primitives::address;
use axum_fix_server::{
    messages::{
        FixMessage, FixMessageBuilder, ServerRequest as AxumServerRequest,
        ServerResponse as AxumServerResponse, SessionId,
    },
    plugins::server_plugin::{SerdePlugin, ServerPlugin, Signer},
    server::Server,
};
use crossbeam::{channel::unbounded, select};
use eyre::{eyre, Report, Result};
use index_maker::{
    core::{bits::Address, logging::log_init},
    init_log,
};
use serde::{Deserialize, Serialize};

fn handle_server_event(event: &ExampleRequest) {
    println!(
        "handle_server_event >> {} {} {}",
        event.session_id, event.address, event.quantity
    );
}

#[derive(Serialize, Deserialize, Debug)]
struct ExampleRequest {
    #[serde(skip)]
    session_id: SessionId,
    address: Address,
    quantity: i32,
}

impl Signer for ExampleRequest {
    fn get_address(&self) -> &Address {
        &self.address
    }
}

#[derive(Serialize, Deserialize, Debug)]
struct ExampleResponse {
    #[serde(skip)]
    session_id: SessionId,
    ack: bool,
    side: String,
}

impl AxumServerRequest for ExampleRequest {
    fn deserialize_from_fix(
        message: FixMessage,
        session_id: SessionId,
    ) -> Result<Self, eyre::Error> {
        println!("{}: {}", session_id, message);
        let mut request: ExampleRequest = serde_json::from_str(&message.to_string())
            .map_err(|e| eyre!("Failed to deserialize FixMessage: {}", e))?;
        request.session_id = session_id.clone();
        println!("deserialize_from_fix: {} {} {}", request.session_id, request.address, request.quantity);
        Ok(request)
    }
}

impl AxumServerResponse for ExampleResponse {
    fn get_session_id(&self) -> &SessionId {
        &self.session_id
    }

    fn serialize_into_fix(&self, builder: FixMessageBuilder) -> Result<FixMessage, eyre::Error> {
        // Serialize the response to JSON
        let json_str = serde_json::to_string(self)
            .map_err(|e| eyre!("Failed to serialize ExampleResponse: {}", e))?;
        // Construct a FixMessage with the serialized data in the body
        println!("serialize_into_fix: {}",json_str);
        Ok(FixMessage (json_str.to_owned()))
        // Ok(FixMessage(
        //     "this is a response, not a good one, but it's something".to_owned(),
        // ))
    }
}

#[tokio::main]
pub async fn main() {
    init_log!();
    let plugin = SerdePlugin::<ExampleRequest, ExampleResponse>::new();
    let fix_server = Server::new_arc(plugin);
    //let plugin = DummyPlugin;
    //let fix_server = Arc::new(RwLock::new(Server::<MyServerRequest, MyServerResponse, DummyPlugin>::new(plugin)));

    let (event_tx, event_rx) = unbounded::<ExampleRequest>();

    fix_server
        .write()
        .await
        .get_multi_observer_mut()
        .set_observer_fn(move |e: ExampleRequest| {
            handle_server_event(&e);
            event_tx.send(e).unwrap();
        });

    let fix_server_weak = Arc::downgrade(&fix_server);

    let handle_server_event_internal = move |e: ExampleRequest| {
        let fix_server = fix_server_weak.upgrade().unwrap();
        fix_server.blocking_read().send_response(ExampleResponse {
            session_id: e.session_id,
            ack: true,
            side: "BUY".to_owned(),
        });
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
