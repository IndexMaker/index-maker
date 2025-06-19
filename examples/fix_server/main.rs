use std::{
    sync::Arc,
    thread::{sleep, spawn},
    time::Duration,
};

use axum_fix_server::{
    messages::{
        FixMessage, FixMessageBuilder, ServerRequest as AxumServerRequest,
        ServerResponse as AxumServerResponse, SessionId,
    },
    plugins::server_plugin::{SerdePlugin, ServerPlugin, Signer},
    server::Server,
};
use crossbeam::{channel::unbounded, select};
use eyre::Result;
use index_maker::{
    core::{bits::Address, logging::log_init},
    init_log,
};

fn handle_server_event(event: &ExampleRequest) {
    println!("{} {} {}", event.session_id, event.address, event.quantity);
}

struct ExampleRequest {
    session_id: SessionId,
    address: Address,
    quantity: i32,
}

impl Signer for ExampleRequest {
    fn get_address(&self) -> &Address {
        &self.address
    }
}

struct ExampleResponse {
    session_id: SessionId,
    ack: bool,
    side: String,
}

impl AxumServerRequest for ExampleRequest {
    fn deserialize_from_fix(
        message: FixMessage,
        session_id: SessionId,
    ) -> Result<Self, eyre::Error> {
        todo!()
    }
}

impl AxumServerResponse for ExampleResponse {
    fn get_session_id(&self) -> &SessionId {
        todo!()
    }

    fn serialize_into_fix(&self, builder: FixMessageBuilder) -> Result<FixMessage, eyre::Error> {
        todo!()
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
