use std::sync::{Arc, RwLock as ComponentLock};

use alloy::primitives::{address, U256};
use alloy::providers::{Provider, ProviderBuilder};
use alloy::rpc::types::TransactionRequest;
use alloy_evm_connector::contracts::ERC20;
use alloy_evm_connector::designation::EvmCollateralDesignation;
use alloy_evm_connector::evm_connector::EvmConnector;
use index_core::collateral::collateral_router::CollateralRouterEvent;
use rust_decimal::dec;
use symm_core::core::functional::IntoObservableSingleFun;
use tokio::sync::watch;
use tracing_subscriber;

#[tokio::main]
async fn main() {
    // Initialize tracing to show logs in terminal
    tracing_subscriber::fmt::init();

    tracing::info!("=== Cross-Chain Bridge Example ===");
    tracing::info!("This example demonstrates cross-chain USDC transfer from Arbitrum to Base using Across protocol");

    // Connect to manually started anvil instance
    let rpc_url = "http://localhost:8545";
    tracing::info!("Connecting to anvil at: {}", rpc_url);

    // Use known anvil default addresses (these are deterministic)
    let admin_address = address!("0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266"); // anvil[0]

    tracing::info!("Using admin address: {:?}", admin_address);

    // USDC contract address on Arbitrum
    let usdc_address = address!("0xaf88d065e77c8cC2239327C5EDb3A432268e5831");
    let whale_address = address!("0xB38e8c17e38363aF6EbdCb3dAE12e0243582891D");

    tracing::info!("=== Setting up USDC funding using whale impersonation ===");

    // Create provider for whale impersonation and balance checking
    let provider = ProviderBuilder::new()
        .connect(rpc_url)
        .await
        .expect("Failed to connect to anvil - make sure anvil is running at http://localhost:8545");

    tracing::info!("Connected to anvil successfully");

    let usdc_contract = ERC20::new(usdc_address, &provider);

    // Check initial admin USDC balance
    let admin_usdc_balance = usdc_contract.balanceOf(admin_address).call().await.unwrap();

    tracing::info!(
        "Admin initial USDC balance: {} USDC",
        admin_usdc_balance / U256::from(1_000_000u64)
    );

    // Check if we need to fund admin with USDC
    let required_usdc = U256::from(10_000_000u64); // 10 USDC
    if admin_usdc_balance < required_usdc {
        tracing::info!("Admin needs USDC funding for bridge operation");

        // Check whale USDC balance
        let whale_balance = usdc_contract.balanceOf(whale_address).call().await.unwrap();
        tracing::info!("Whale USDC balance: {} (raw)", whale_balance);

        if whale_balance < U256::from(100_000_000u64) {
            tracing::error!("Whale account doesn't have enough USDC. Make sure anvil is forked from Arbitrum with: anvil --fork-url https://arb1.lava.build");
            return;
        }

        // Impersonate the whale account
        let impersonate_cmd = format!(
            r#"{{"jsonrpc":"2.0","method":"anvil_impersonateAccount","params":["{}"],"id":1}}"#,
            whale_address
        );

        let client = std::process::Command::new("curl")
            .arg("-X")
            .arg("POST")
            .arg("-H")
            .arg("Content-Type: application/json")
            .arg("-d")
            .arg(&impersonate_cmd)
            .arg(rpc_url)
            .output()
            .expect("Failed to execute curl command");

        if !client.status.success() {
            tracing::error!("Failed to impersonate whale account");
            return;
        }

        tracing::info!(
            "Successfully impersonated whale account: {:?}",
            whale_address
        );

        // Transfer 100 USDC from whale to admin
        let transfer_amount = U256::from(100_000_000u64); // 100 USDC
        tracing::info!(
            "Transferring {} USDC from whale to admin...",
            transfer_amount / U256::from(1_000_000u64)
        );

        let transfer_call = usdc_contract.transfer(admin_address, transfer_amount);
        let transfer_calldata = transfer_call.calldata().clone();

        let transfer_tx = TransactionRequest::default()
            .to(usdc_address)
            .input(transfer_calldata.into())
            .from(whale_address)
            .gas_limit(100000);

        match provider.send_transaction(transfer_tx).await {
            Ok(pending_tx) => match pending_tx.get_receipt().await {
                Ok(receipt) => {
                    tracing::info!("USDC transfer completed: {:?}", receipt.transaction_hash);
                }
                Err(e) => {
                    tracing::error!("Transfer receipt failed: {}", e);
                    return;
                }
            },
            Err(e) => {
                tracing::error!("Transfer transaction failed: {}", e);
                return;
            }
        }

        // Verify the transfer
        let admin_balance_after = usdc_contract.balanceOf(admin_address).call().await.unwrap();
        tracing::info!(
            "Admin USDC balance after funding: {} USDC",
            admin_balance_after / U256::from(1_000_000u64)
        );
    } else {
        tracing::info!("Admin already has sufficient USDC balance");
    }

    tracing::info!("=== Starting Bridge Operation ===");

    // Create the EvmConnector first (new architecture)
    let mut connector = EvmConnector::new();

    // Start the connector (this initializes the arbiter)
    tracing::info!("🚀 Starting EvmConnector...");
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

    // Create designations with admin address
    let source = Arc::new(ComponentLock::new(EvmCollateralDesignation::arbitrum_usdc(
        admin_address,
    )));

    let destination = Arc::new(ComponentLock::new(EvmCollateralDesignation::base_usdc(
        admin_address,
    )));

    // Create bridge using the generic method (it will automatically select Across bridge for cross-chain)
    let bridge = connector.create_bridge(source, destination);

    let chain_id = 42161;
    let client_order_id = "C01".into();
    let route_from = "ARBITRUM".into();
    let route_to = "BASE".into();
    let amount = dec!(10000000.0); // 10 USDC (6 decimals) = 10,000,000 wei
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
            admin_address,
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

    // Properly shutdown the connector to avoid the error
    tracing::info!("Shutting down EvmConnector...");
    connector.stop().await.expect("Failed to stop EvmConnector");
    tracing::info!("EvmConnector shutdown complete");
    tracing::info!("");
    tracing::info!("Cross-chain bridge operation completed successfully!");
    tracing::info!("Transferred 10 USDC from Arbitrum to Base using Across protocol");
}
