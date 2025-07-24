use std::sync::{Arc, RwLock as ComponentLock};

use alloy::primitives::address;
use alloy_evm_connector::across_deposit::{USDC_ARBITRUM_ADDRESS, USDC_BASE_ADDRESS};
use alloy_evm_connector::designation::EvmCollateralDesignation;
use alloy_evm_connector::evm_connector::EvmConnector;
use index_core::collateral::collateral_router::{CollateralBridge, CollateralRouterEvent};
use rust_decimal::dec;
use symm_core::core::functional::{IntoObservableSingleFun, IntoObservableSingleVTable};
use symm_core::{core::logging::log_init, init_log};
use tokio::sync::watch;

#[tokio::main]
async fn main() {
    // Initialize logging to show in terminal
    init_log!();

    // Create the EvmConnector first (new architecture)
    let mut connector = EvmConnector::new();

    // Start the connector (this initializes the arbiter)
    tracing::info!("Starting EvmConnector...");
    connector.start().expect("Failed to start EvmConnector");

    // Connect to chains
    tracing::info!("Connecting to Arbitrum and Base chains...");
    connector
        .connect_arbitrum()
        .await
        .expect("Failed to connect to Arbitrum");
    connector
        .connect_base()
        .await
        .expect("Failed to connect to Base");

    // Give time for chain operations to be added
    tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
    tracing::info!("EvmConnector and chains initialized");

    // Create designations with simplified factory methods
    let source = Arc::new(ComponentLock::new(EvmCollateralDesignation::arbitrum_usdc(
        USDC_ARBITRUM_ADDRESS,
    )));

    let destination = Arc::new(ComponentLock::new(EvmCollateralDesignation::base_usdc(
        USDC_BASE_ADDRESS,
    )));

    // Create bridge using the generic method (it will automatically select Across bridge for cross-chain)
    let bridge = connector.create_bridge(source, destination);

    let chain_id = 42161;
    let address = address!("0xd8da6bf26964af9d7eed9e03e53415d37aa96045");
    let client_order_id = "C01".into();
    let route_from = "ARBITRUM".into();
    let route_to = "BASE".into();
    let amount = dec!(10000000.0); // 10 USDC (6 decimals) = 1,000,000 wei
    let cumulative_fee = dec!(0.0);

    let (end_tx, mut end_rx) = watch::channel(false);

    bridge
        .write()
        .unwrap()
        .set_observer_fn(move |event: CollateralRouterEvent| match event {
            CollateralRouterEvent::HopComplete {
                chain_id,
                address,
                client_order_id,
                timestamp,
                source,
                destination,
                route_from,
                route_to,
                amount,
                fee,
            } => {
                tracing::info!(
                    "(evm-bridge-main) Hop Complete {} {} {} {} {} {} {} {} {} {}",
                    chain_id,
                    address,
                    client_order_id,
                    timestamp,
                    source,
                    destination,
                    route_from,
                    route_to,
                    amount,
                    fee,
                );
                end_tx.send(true).expect("Failed to send ok");
            }
        });

    bridge
        .write()
        .unwrap()
        .transfer_funds(
            chain_id,
            address,
            client_order_id,
            route_from,
            route_to,
            amount,
            cumulative_fee,
        )
        .expect("Failed to schedule funds transfer");

    end_rx
        .changed()
        .await
        .expect("Failed to await for transfer");
}
