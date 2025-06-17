use std::{
    sync::Arc,
    thread::{sleep, spawn},
    time::Duration,
};

use axum_fix_server::{
    server::Server,
    plugins::server_plugin::{DummyPlugin, ServerPlugin},
    messages::{MyServerRequest, MyServerResponse},
};
use crossbeam::{channel::unbounded, select};
use index_maker::{
    core::functional::IntoObservableMany,
    server::server::{
        NewIndexOrderNakReason, ServerEvent, ServerResponse, ServerResponseReason,
    },
};
use parking_lot::RwLock;

fn handle_server_event(event: &Arc<ServerEvent>) {
    match event.as_ref() {
        ServerEvent::NewIndexOrder {
            chain_id,
            address,
            client_order_id,
            symbol,
            side,
            collateral_amount,
            timestamp,
        } => {
            println!(
                "(fix-server-main) NewIndexOrder {} {} {} {} {:?} {} {}",
                chain_id, address, client_order_id, symbol, side, collateral_amount, timestamp
            );
        }
        ServerEvent::CancelIndexOrder {
            chain_id,
            address,
            client_order_id,
            symbol,
            collateral_amount,
            timestamp,
        } => {
            println!(
                "(fix-server-main) CancelIndexOrder {} {} {} {} {} {}",
                chain_id, address, client_order_id, symbol, collateral_amount, timestamp
            );
        }
        ServerEvent::NewQuoteRequest {
            chain_id,
            address,
            client_quote_id,
            symbol,
            side,
            collateral_amount,
            timestamp,
        } => {
            println!(
                "(fix-server-main) NewQuoteRequest {} {} {} {} {:?} {} {}",
                chain_id, address, client_quote_id, symbol, side, collateral_amount, timestamp
            );
        }
        ServerEvent::CancelQuoteRequest {
            chain_id,
            address,
            client_quote_id,
            symbol,
            timestamp,
        } => {
            println!(
                "(fix-server-main) CancelQuoteRequest {} {} {} {} {}",
                chain_id, address, client_quote_id, symbol, timestamp
            );
        }
        ServerEvent::AccountToCustody => todo!(),
        ServerEvent::CustodyToAccount => todo!(),
    }
}

// Placeholder for plugin; replace with your actual plugin implementation
struct MyPlugin;
// Implement ServerPlugin for MyPlugin (minimal example)
impl ServerPlugin<ServerEvent, ServerResponse> for MyPlugin {
    fn process_incoming(&self, message: String, session_id: SessionId) -> Result<ServerEvent, Report> {
        let fix_message = FixMessage(message);
        MyServerRequest::deserialize_from_fix(fix_message, session_id)
    }

    fn process_outgoing(&self, response: &dyn ServerResponse) -> Result<String, Report> {
        let builder = FixMessageBuilder::new();
        response.serialize_into_fix(builder).map(|msg| msg.0)
    }
}



#[tokio::main]
pub async fn main() {
    let plugin = MyPlugin;
    let fix_server = Arc::new(RwLock::new(
        Server::<ServerEvent, ServerResponse, MyPlugin>::new(plugin)
    ));
    //let plugin = DummyPlugin;
    //let fix_server = Arc::new(RwLock::new(Server::<MyServerRequest, MyServerResponse, DummyPlugin>::new(plugin)));

    let (event_tx, event_rx) = unbounded::<Arc<ServerEvent>>();

    fix_server
        .write()
        .get_multi_observer_mut()
        .add_observer_fn(move |e: &Arc<ServerEvent>| {
            handle_server_event(e);
            event_tx.send(e.clone()).unwrap();
        });

    let fix_server_weak = Arc::downgrade(&fix_server);

    let handle_server_event_internal = move |e: Arc<ServerEvent>| {
        let fix_server = fix_server_weak.upgrade().unwrap();

        match e.as_ref() {
            ServerEvent::NewIndexOrder {
                chain_id,
                address,
                client_order_id,
                symbol,
                side: _,
                collateral_amount: _,
                timestamp,
            } => {
                if symbol != "I1" {
                    fix_server
                        .write()
                        .respond_with(ServerResponse::NewIndexOrderNak {
                            chain_id: *chain_id,
                            address: *address,
                            client_order_id: client_order_id.clone(),
                            reason: ServerResponseReason::User(
                                NewIndexOrderNakReason::OtherReason {
                                    detail: "Bad symbol".to_owned(),
                                },
                            ),
                            timestamp: *timestamp,
                        });
                } else {
                    fix_server
                        .write()
                        .respond_with(ServerResponse::NewIndexOrderAck {
                            chain_id: *chain_id,
                            address: *address,
                            client_order_id: client_order_id.clone(),
                            timestamp: *timestamp,
                        });
                }
            }
            ServerEvent::CancelIndexOrder {
                chain_id,
                address,
                client_order_id,
                symbol: _,
                collateral_amount: _,
                timestamp,
            } => fix_server
                .write()
                .respond_with(ServerResponse::CancelIndexOrderAck { // add NAK case
                    chain_id: *chain_id,
                    address: *address,
                    client_order_id: client_order_id.clone(),
                    timestamp: *timestamp,
                }),
            ServerEvent::NewQuoteRequest {
                chain_id,
                address,
                client_quote_id,
                symbol: _,
                side: _,
                collateral_amount: _,
                timestamp,
            } => {
                fix_server
                    .write()
                    .respond_with(ServerResponse::NewIndexQuoteAck { // add NAK case
                        chain_id: *chain_id,
                        address: *address,
                        client_quote_id: client_quote_id.clone(),
                        timestamp: *timestamp,
                    });
            }
            ServerEvent::CancelQuoteRequest {
                chain_id,
                address,
                client_quote_id,
                symbol: _,
                timestamp,
            } => fix_server
                .write()
                .respond_with(ServerResponse::CancelIndexQuoteAck { // add NAK case
                    chain_id: *chain_id,
                    address: *address,
                    client_quote_id: client_quote_id.clone(),
                    timestamp: *timestamp,
                }),
            ServerEvent::AccountToCustody => {}
            ServerEvent::CustodyToAccount => {}
        };
    };

    spawn(move || {
        loop {
            select!(
                recv(event_rx) -> res => {
                    handle_server_event_internal(res.unwrap())
                },
            )
        }
    });

    fix_server.write().start_server(); //< should launch async task, and return immediatelly

    sleep(Duration::from_secs(600));
}
