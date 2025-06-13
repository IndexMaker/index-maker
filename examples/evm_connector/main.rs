use std::{thread::sleep, time::Duration};

use index_maker::{
    blockchain::{chain_connector::ChainNotification, evm::{evm_connector::EvmConnector}},
    core::functional::IntoObservableSingle,
};
use itertools::Itertools;

pub fn handle_chain_event(event: ChainNotification) {
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
    let mut evm_connector = EvmConnector::new();

    evm_connector
        .get_single_observer_mut()
        .set_observer_fn(handle_chain_event);

    evm_connector.connect(); //< should launch async task, and return immediatelly

    // test that these are working
    // evm_connector.mint_index(chain_id, symbol, quantity, receipient, execution_price, execution_time);
    // evm_connector.burn_index(chain_id, symbol, quantity, receipient);
    // evm_connector.withdraw(chain_id, receipient, amount, execution_price, execution_time);

    sleep(Duration::from_secs(600));
}
