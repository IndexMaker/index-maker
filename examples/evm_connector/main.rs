use std::{
    collections::HashMap,
    sync::Arc,
    thread::{sleep, spawn},
    time::Duration,
};

use alloy_evm_connector::evm_connector::EvmConnector;
use crossbeam::{channel::unbounded, select};
use index_maker::{
    blockchain::chain_connector::{ChainConnector, ChainNotification},
    core::functional::IntoObservableSingle,
    index::basket::Basket,
};
use itertools::Itertools;
use parking_lot::RwLock;
use rust_decimal::dec;

pub fn handle_chain_event(event: &ChainNotification) {
    match event {
        ChainNotification::CuratorWeightsSet(symbol, basket_definition) => {
            println!(
                "(evm-connector-main) CuratorWeightsSet {}: {}",
                symbol,
                basket_definition
                    .weights
                    .iter()
                    .map(|w| format!("{}:{}", w.asset.name, w.weight))
                    .join(", ")
            );
        }
        ChainNotification::Deposit {
            chain_id,
            address,
            amount,
            timestamp,
        } => {
            println!(
                "(evm-connector-main) Deposit {} {} {} {}",
                chain_id, address, amount, timestamp
            );
        }
        ChainNotification::WithdrawalRequest {
            chain_id,
            address,
            amount,
            timestamp,
        } => {
            println!(
                "(evm-connector-main) WithdrawalRequest {} {} {} {}",
                chain_id, address, amount, timestamp
            );
        }
    }
}

#[tokio::main]
pub async fn main() {
    let evm_connector = Arc::new(RwLock::new(EvmConnector::new()));

    let (event_tx, event_rx) = unbounded::<ChainNotification>();

    let evm_connector_weak = Arc::downgrade(&evm_connector);

    let handle_event_internal = move |e: ChainNotification| {
        let evm_connector = evm_connector_weak.upgrade().unwrap();
        match e {
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

    evm_connector
        .write()
        .get_single_observer_mut()
        .set_observer_fn(move |e| {
            handle_chain_event(&e);
            event_tx.send(e).unwrap();
        });

    evm_connector.write().connect(); //< should launch async task, and return immediatelly

    spawn(move || loop {
        select! {
            recv(event_rx) -> res => {
                handle_event_internal(res.unwrap());
            }
        }
    });

    sleep(Duration::from_secs(600));
}
