use std::sync::{Arc, RwLock as ComponentLock};

use alloy::primitives::address;
use alloy_evm_connector::evm_bridge::{EvmCollateralBridge, EvmCollateralDesignation};
use index_core::collateral::collateral_router::{CollateralBridge, CollateralRouterEvent};
use rust_decimal::dec;
use symm_core::core::functional::{IntoObservableSingleFun, IntoObservableSingleVTable};
use tokio::sync::watch;

#[tokio::main]
async fn main() {
    let source = Arc::new(ComponentLock::new(EvmCollateralDesignation {
        name: "ARBITRUM".into(),
        collateral_symbol: "USDC".into(),
        full_name: "EVM:ARBITRUM:USDC".into(),
    }));

    let destination = Arc::new(ComponentLock::new(EvmCollateralDesignation {
        name: "BASE".into(),
        collateral_symbol: "USDC".into(),
        full_name: "EVM:BASE:USDC".into(),
    }));

    let bridge = EvmCollateralBridge::new_arc(source, destination);

    // Initialize the new arbiter architecture
    println!("🔧 Initializing arbiter system...");
    bridge
        .write()
        .unwrap()
        .start_arbiter()
        .expect("Failed to start arbiter system");

    // Give time for chain operations to be added
    tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
    println!("✅ Arbiter system initialized");

    let chain_id = 42161;
    let address = address!("0xd8da6bf26964af9d7eed9e03e53415d37aa96045");
    let client_order_id = "C01".into();
    let route_from = "ARBITRUM".into();
    let route_to = "BASE".into();
    let amount = dec!(1000000.0); // 1 USDC (6 decimals) = 1,000,000 wei
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
                println!(
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
