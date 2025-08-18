use std::{
    collections::HashMap,
    sync::Arc,
    thread::{sleep, spawn},
    time::Duration,
};

use alloy_evm_connector::evm_connector::EvmConnector;
use crossbeam::{
    channel::{bounded, unbounded},
    select,
};
use index_core::{
    blockchain::chain_connector::{ChainConnector, ChainNotification},
    index::basket::Basket,
};
use parking_lot::RwLock;
use rust_decimal::dec;
use serde_json::json;
use symm_core::{
    core::{functional::IntoObservableSingleFun, logging::log_init},
    init_log,
};
use tokio::signal::unix::{signal, SignalKind};

pub fn handle_chain_event(event: &ChainNotification) {
    match event {
        ChainNotification::ChainConnected {
            chain_id,
            timestamp,
        } => {
            tracing::info!(%chain_id, %timestamp, "Chain connected");
        }
        ChainNotification::ChainDisconnected {
            chain_id,
            reason,
            timestamp,
        } => {
            tracing::info!(%chain_id, %timestamp, "Chain disconnected: {}", reason);
        }
        ChainNotification::CuratorWeightsSet(symbol, basket_definition) => {
            tracing::info!(
                    %symbol, basket_definition = %json!(basket_definition.weights),
                    "Curator weights set");
        }
        ChainNotification::Deposit {
            chain_id,
            address,
            amount,
            timestamp,
        } => {
            tracing::info!(%chain_id, %address, %amount, %timestamp, "Deposit");
        }
        ChainNotification::WithdrawalRequest {
            chain_id,
            address,
            amount,
            timestamp,
        } => {
            tracing::info!(%chain_id, %address, %amount, %timestamp, "Withdrawal request");
        }
    }
}

#[tokio::main]
pub async fn main() {
    init_log!();

    let evm_connector = Arc::new(RwLock::new(EvmConnector::new()));

    let (event_tx, event_rx) = unbounded::<ChainNotification>();

    let evm_connector_weak = Arc::downgrade(&evm_connector);

    let handle_event_internal = move |e: ChainNotification| {
        let evm_connector = evm_connector_weak.upgrade().unwrap();
        match e {
            ChainNotification::ChainConnected {
                chain_id,
                timestamp,
            } => {}
            ChainNotification::ChainDisconnected {
                chain_id,
                reason,
                timestamp,
            } => {}
            ChainNotification::CuratorWeightsSet(symbol, basket_definition) => {
                // When we receive curator weights, we respond with solver weights set
                let individual_prices = HashMap::from_iter([
                    ("A1".into(), dec!(100000.0)),
                    ("A2".into(), dec!(1000.0)),
                    ("A3".into(), dec!(10.0)),
                    ("A4".into(), dec!(100.0)),
                ]);
                let basket = Arc::new(
                    Basket::new_with_prices(basket_definition, &individual_prices, dec!(1000.0))
                        .unwrap(),
                );
                evm_connector.write().solver_weights_set(symbol, basket);
            }
            ChainNotification::Deposit {
                chain_id,
                address,
                amount,
                timestamp,
            } => {
                // When we receive deposit, we respond with mint and burn
                evm_connector.write().mint_index(
                    chain_id,
                    "I1".into(),
                    dec!(2.0),
                    address,
                    amount,
                    timestamp,
                );
                evm_connector
                    .write()
                    .burn_index(chain_id, "I1".into(), dec!(1.0), address);
            }
            ChainNotification::WithdrawalRequest {
                chain_id,
                address,
                amount,
                timestamp,
            } => {
                // When we receive withdrawal request, we respond with withdraw
                evm_connector
                    .write()
                    .withdraw(chain_id, address, amount, dec!(1000.0), timestamp);
            }
        }
    };

    evm_connector.write().set_observer_fn(move |e| {
        tracing::info!("Received chain event!");
        handle_chain_event(&e);
        if let Err(err) = event_tx.send(e) {
            tracing::warn!("Failed to send EVM event into channel: {:?}", err);
        }
    });

    tracing::info!("Starting EVM connector...");

    evm_connector
        .write()
        .start()
        .expect("Failed to start EVM connector");

    tracing::info!("Connecting EVM connector...");

    let anvil_url = String::from("http://127.0.0.1:8545");
    let anvil_default_pk =
        String::from("0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80");

    evm_connector
        .write()
        .connect_chain(42161, anvil_url, anvil_default_pk)
        .await
        .expect("Failed to connect to ARBITRUM");

    let (stop_tx, stop_rx) = bounded(1);

    std::thread::spawn(move || {
        tracing::info!("Listening for EVM events...");
        loop {
            select! {
                recv(stop_rx) -> _ => {
                    break;
                }
                recv(event_rx) -> res => {
                    handle_event_internal(res.unwrap());
                }
            }
        }
        tracing::info!("Finished listening for EVM events.");
    });

    let mut sigint = signal(SignalKind::interrupt()).expect("Failed to obtain SIGINT");
    let _ = sigint.recv().await;

    if let Err(err) = stop_tx.send(1) {
        tracing::warn!("Failed to send stop: {:?}", err);
    }

    // Properly shutdown the connector to avoid the error
    tracing::info!("Shutting down EvmConnector...");
    evm_connector
        .write()
        .stop()
        .await
        .expect("Failed to stop EvmConnector");

    tracing::info!("EvmConnector shutdown complete");
}
