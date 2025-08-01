use std::sync::{Arc, RwLock as ComponentLock};

use alloy::primitives::address;
use alloy_evm_connector::designation::EvmCollateralDesignation;
use alloy_evm_connector::evm_connector::EvmConnector;

#[tokio::main]
async fn main() {
    // Initialize tracing
    tracing_subscriber::fmt::init();

    // Create the EvmConnector
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

    // Test Case 1: Cross-chain transfer (ARBITRUM -> BASE)
    tracing::info!("Test Case 1: Cross-chain transfer (ARBITRUM -> BASE)");

    let wallet_address = address!("0xC0D3CB2E7452b8F4e7710bebd7529811868a85dd");
    let arbitrum_usdc = Arc::new(ComponentLock::new(EvmCollateralDesignation::arbitrum_usdc(
        wallet_address,
    )));

    let base_usdc = Arc::new(ComponentLock::new(EvmCollateralDesignation::base_usdc(
        wallet_address,
    )));

    let cross_chain_bridge = connector.create_bridge(arbitrum_usdc.clone(), base_usdc.clone());
    tracing::info!("Created bridge for cross-chain transfer");

    // Test Case 2: Same-chain transfer (BASE -> BASE)
    tracing::info!("Test Case 2: Same-chain transfer (BASE -> BASE)");

    let different_wallet_address = address!("0x833589fcd6edb6e08f4c7c32d4f71b54bda02913");
    let base_usdc_2 = Arc::new(ComponentLock::new(EvmCollateralDesignation::base_usdc(
        different_wallet_address, // Different wallet address for demo
    )));

    let same_chain_bridge = connector.create_bridge(base_usdc.clone(), base_usdc_2.clone());
    tracing::info!("Created bridge for same-chain transfer");

    tracing::info!("All tests completed successfully!");
    tracing::info!("The generic create_bridge() method automatically selected:");
    tracing::info!("- Across bridge for cross-chain transfers");
    tracing::info!("- ERC20 bridge for same-chain transfers");
    tracing::info!("- Based on the designation details stored in EvmCollateralDesignation");

    // Properly shutdown the connector to avoid the error
    tracing::info!("Shutting down EvmConnector...");
    connector.stop().await.expect("Failed to stop EvmConnector");
    tracing::info!("EvmConnector shutdown complete");
}
