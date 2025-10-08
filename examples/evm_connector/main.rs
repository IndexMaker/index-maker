use alloy::primitives::{self, address, U256};
use alloy_evm_connector::{
    designation::{self, EvmCollateralDesignation},
    evm_connector::EvmConnector,
};
use crossbeam::{
    channel::{bounded, unbounded},
    select,
};
use index_core::{
    blockchain::chain_connector::{ChainConnector, ChainNotification},
    collateral::collateral_router::{CollateralDesignation, CollateralRouterEvent},
    index::basket::Basket,
};
use parking_lot::RwLock;
use rust_decimal::dec;
use serde_json::json;
use std::{
    collections::HashMap,
    sync::Arc,
    thread::{sleep, spawn},
};
use symm_core::{
    core::{
        self,
        bits::{self, Amount, ClientOrderId},
        functional::{IntoObservableSingleFun, IntoObservableSingleFunRef, IntoObservableSingleVTable},
        logging::log_init,
    },
    init_log,
};

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
            seq_num,
            affiliate1,
            affiliate2,
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
    // Init logging
    init_log!();

    let evm_connector = Arc::new(RwLock::new(EvmConnector::new()));
    let (event_tx, event_rx) = crossbeam::channel::unbounded::<ChainNotification>();
    let evm_connector_weak = Arc::downgrade(&evm_connector);

    let custody_a = address!("0x70997970C51812dc3A010C7d01b50e0d17dc79C8");
    let custody_b = address!("0x3C44CdDdB6a900fa2b585dd299e03d12FA4293BC");

    // Precompute route/amounts for bridge transfers
    let chain_id = alloy_evm_connector::config::EvmConnectorConfig::default()
        .get_chain_id("arbitrum")
        .expect("chain id");
    let client_order_id = ClientOrderId::from("TEST_ERC20_FROM_DEPOSIT");
    let bridge_amount: Amount = dec!(1.0);
    let cumulative_fee: Amount = dec!(0.0);

    // Observer to forward events to our internal handler channel
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

    // === Create the bridge IMMEDIATELY after connector is up (bridges aren't dynamic) ===
    // Use std::sync::RwLock for designations (as in the bridge sample)
    let src_custody = Arc::new(
        EvmCollateralDesignation::arbitrum_usdc_with_name(custody_a, "CUSTODY_A"),
    );
    let dst_custody = Arc::new(
        EvmCollateralDesignation::arbitrum_usdc_with_name(custody_b, "CUSTODY_B"),
    );

    let src_custody_name = src_custody.get_full_name();
    let dst_custody_name = dst_custody.get_full_name();

    let bridge = evm_connector
        .write()
        .create_bridge(src_custody.clone(), dst_custody.clone());

    tracing::info!(
        "BRIDGE_CREATED: from {:?} (Arb USDC) -> {:?} (Arb USDC)",
        custody_a,
        custody_b
    );

    bridge.set_observer_fn(|e: CollateralRouterEvent| match e {
        CollateralRouterEvent::HopComplete {
            chain_id,
            address,
            client_order_id: _,
            timestamp: _,
            source,
            destination,
            route_from: _,
            route_to: _,
            amount,
            fee,
            status,
        } => {
            tracing::info!(%chain_id, %address, %amount, %source, %destination, %fee,
                    "Collateral routing hop complete");
        }
    });

    // Capture for event loop
    let bridge_for_events = bridge.clone();
    let custody_a_watch = custody_a;

    let (stop_tx, stop_rx) = crossbeam::channel::bounded(1);
    std::thread::spawn(move || {
        tracing::info!("Listening for EVM events...");
        loop {
            select! {
                recv(stop_rx) -> _ => {
                    break;
                }
                recv(event_rx) -> res => {
                    let e = res.unwrap();
                    let Some(evm_connector_arc) = evm_connector_weak.upgrade() else {
                        tracing::warn!("EVM connector dropped; stopping listener.");
                        break;
                    };

                    match e {
                        ChainNotification::ChainConnected { .. } => {}
                        ChainNotification::ChainDisconnected { .. } => {}
                        ChainNotification::CuratorWeightsSet(symbol, basket_definition) => {
                            let individual_prices = basket_definition.weights
                                .iter()
                                .map(|w| (w.asset.ticker.clone(), dec!(100.0)))
                                .collect();

                            let basket = Arc::new(
                                Basket::new_with_prices(basket_definition, &individual_prices, dec!(1000.0))
                                    .unwrap(),
                            );
                            evm_connector_arc.write().solver_weights_set(symbol, basket);
                        }
                        ChainNotification::Deposit { chain_id: ev_chain_id, seq_num, affiliate1, affiliate2, address, amount, timestamp } => {
                            if address == custody_a_watch {
                                match bridge_for_events.transfer_funds(
                                    chain_id,
                                    address,
                                    client_order_id.clone(),
                                    src_custody_name.clone(),
                                    dst_custody_name.clone(),
                                    bridge_amount.clone(),
                                    cumulative_fee.clone(),
                                ) {
                                    Ok(_)  => tracing::info!("BRIDGE_TRANSFER_SENT: {:?} USDC, CID={}", bridge_amount, client_order_id),
                                    Err(e) => tracing::error!("BRIDGE_TRANSFER_ERROR: {:?}", e),
                                }
                            } else {
                                tracing::info!("DEPOSIT_NON_MATCH: address {:?} != custody A {:?} -> skipping bridge; running demo mint/burn", address, custody_a_watch);
                                evm_connector_arc.write().mint_index(
                                    ev_chain_id,
                                    "I1".into(),
                                    dec!(2.0),
                                    address,
                                    U256::ZERO,
                                    amount,
                                    timestamp,
                                );
                                evm_connector_arc.write().burn_index(ev_chain_id, "I1".into(), dec!(1.0), address);
                            }
                        }
                        ChainNotification::WithdrawalRequest { chain_id, address, amount, timestamp } => {
                            evm_connector_arc
                                .write()
                                .withdraw(chain_id, address, amount, dec!(1000.0), timestamp);
                        }
                    }
                }
            }
        }
        tracing::info!("Finished listening for EVM events.");
    });

    // === Wait for SIGINT and graceful shutdown ===
    let mut sigint = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())
        .expect("Failed to obtain SIGINT");
    let _ = sigint.recv().await;

    if let Err(err) = stop_tx.send(1) {
        tracing::warn!("Failed to send stop: {:?}", err);
    }

    tracing::info!("Shutting down EvmConnector...");
    evm_connector
        .write()
        .stop()
        .await
        .expect("Failed to stop EvmConnector");
    tracing::info!("EvmConnector shutdown complete");
}
