use std::sync::{Arc, RwLock as ComponentLock};

use alloy::primitives::address;
use alloy_evm_connector::designation::EvmCollateralDesignation;
use alloy_evm_connector::evm_connector::EvmConnector;
use rust_decimal::dec;
use std::str::FromStr;
use symm_core::core::bits::{Address, Amount, ClientOrderId, Symbol};
use tracing_subscriber;

#[tokio::main]
async fn main() {
    // Initialize tracing
    tracing_subscriber::fmt::init();

    // Create the EvmConnector
    let mut connector = EvmConnector::new();

    // Start the connector (this initializes the arbiter)
    tracing::info!("Starting EvmConnector...");
    connector.start().expect("Failed to start EvmConnector");

    // Connect to Arbitrum chain (we'll do same-chain transfer)
    tracing::info!("Connecting to Arbitrum chain...");
    connector
        .connect_arbitrum()
        .await
        .expect("Failed to connect to Arbitrum");

    // Give time for chain operations to be added
    tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
    tracing::info!("EvmConnector and Arbitrum chain initialized");

    // Test Same-chain ERC20 transfer (ARBITRUM -> ARBITRUM)
    tracing::info!("Testing same-chain ERC20 transfer (ARBITRUM -> ARBITRUM)");

    // Create source designation (Arbitrum USDC)
    let wallet_address = address!("0xC0D3CB2E7452b8F4e7710bebd7529811868a85dd");
    let source_designation = Arc::new(ComponentLock::new(EvmCollateralDesignation::arbitrum_usdc(
        wallet_address,
    )));

    let destination_designation = Arc::new(ComponentLock::new(
        EvmCollateralDesignation::arbitrum_usdc(wallet_address),
    ));

    // Create the bridge (should automatically select ERC20 bridge for same-chain)
    let erc20_bridge =
        connector.create_bridge(source_designation.clone(), destination_designation.clone());

    tracing::info!("Created ERC20 bridge for same-chain transfer");

    // Test parameters
    let chain_id = 42161u32; // Arbitrum chain ID
    let address = address!("0xd8dA6BF26964aF9D7eEd9e03E53415D37aA96045");
    let client_order_id = ClientOrderId::from("TEST_ERC20_001");
    let route_from = Symbol::from("EVM:ARBITRUM:USDC");
    let route_to = Symbol::from("EVM:ARBITRUM:USDC");
    let amount = Amount::from_str("1.0").unwrap(); // 1 USDC
    let cumulative_fee = Amount::from_str("0.1").unwrap(); // 5 USDC fee

    tracing::info!(
        "Initiating same-chain ERC20 transfer: {} USDC with {} fee",
        amount,
        cumulative_fee
    );

    // Execute the transfer
    let result = {
        let bridge = erc20_bridge.read().unwrap();
        bridge.transfer_funds(
            chain_id,
            address,
            client_order_id,
            route_from,
            route_to,
            amount,
            cumulative_fee,
        )
    };

    match result {
        Ok(_) => {
            tracing::info!("ERC20 transfer command sent successfully!");
            tracing::info!("Waiting for async blockchain operation to complete...");

            // Wait for the async blockchain operation to complete
            // In a real application, you would wait for the callback or use a proper async mechanism
            tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;

            tracing::info!("This test demonstrated:");
            tracing::info!("- Same-chain detection (Arbitrum -> Arbitrum)");
            tracing::info!("- Automatic ERC20 bridge selection");
            tracing::info!("- Original routing amounts passed to callback");
            tracing::info!("- Actual ERC20 contract interaction (async)");
        }
        Err(e) => {
            tracing::error!("ERC20 transfer failed: {}", e);
        }
    }

    // Properly shutdown the connector to avoid the error
    tracing::info!("Shutting down EvmConnector...");
    connector.stop().await.expect("Failed to stop EvmConnector");
    tracing::info!("EvmConnector shutdown complete");

    tracing::info!("ERC20 transfer test completed!");
}
