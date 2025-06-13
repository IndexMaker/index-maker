use std::{
    sync::Arc,
    thread::{sleep, spawn},
    time::Duration,
};

use axum_fix_server::fix_server::FixServer;
use crossbeam::{channel::unbounded, select};
use index_maker::{
    core::functional::IntoObservableMany,
    server::server::{
        NewIndexOrderNakReason, Server, ServerEvent, ServerResponse, ServerResponseReason,
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

#[tokio::main]
pub async fn main() {
    let fix_server = Arc::new(RwLock::new(FixServer::new()));

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
