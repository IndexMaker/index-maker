use std::sync::{Arc, RwLock as ComponentLock};

use alloy::primitives::address;
use alloy::providers::{Provider, ProviderBuilder};
use alloy::rpc::types::TransactionRequest;
use alloy_evm_connector::config::EvmConnectorConfig;
use alloy_evm_connector::contracts::ERC20;
use alloy_evm_connector::designation::EvmCollateralDesignation;
use alloy_evm_connector::evm_connector::EvmConnector;
use alloy_evm_connector::utils::{IntoAmount, IntoEvmAmount};
use index_core::collateral::collateral_router::CollateralRouterEvent;
use rust_decimal::dec;
use symm_core::core::bits::Amount;
use symm_core::core::functional::IntoObservableSingleFun;
use tokio::sync::watch;
use tracing_subscriber;

#[tokio::main]
async fn main() {
    // Initialize tracing to show logs in terminal
    tracing_subscriber::fmt::init();

    tracing::info!("=== Cross-Chain Bridge Example ===");
    tracing::info!("Starting Arbitrum -> Base bridge operation...");

    let config = EvmConnectorConfig::default();
    let rpc_url = EvmConnectorConfig::get_default_rpc_url();
    let admin_address = EvmConnectorConfig::get_default_sender_address();

    // USDC contract address on Arbitrum
    let usdc_address = config.get_usdc_address("arbitrum").unwrap();
    let whale_address = address!("0xB38e8c17e38363aF6EbdCb3dAE12e0243582891D");

    // Setup USDC funding using whale impersonation;

    // Create provider for whale impersonation and balance checking
    let provider = ProviderBuilder::new()
        .connect(&rpc_url)
        .await
        .expect("Failed to connect to anvil - make sure anvil is running at http://localhost:8545");

    let usdc_contract = ERC20::new(usdc_address, &provider);

    // Check initial admin USDC balance
    let admin_usdc_balance = usdc_contract.balanceOf(admin_address).call().await.unwrap();

    tracing::info!(
        "Admin initial USDC balance: {} USDC",
        admin_usdc_balance.into_amount_usdc().unwrap()
    );

    // Check if we need to fund admin with USDC
    let required_usdc = dec!(10.0).into_evm_amount_usdc().unwrap(); // 10 USDC
    if admin_usdc_balance < required_usdc {
        // Check whale USDC balance
        let whale_balance = usdc_contract.balanceOf(whale_address).call().await.unwrap();

        if whale_balance < dec!(100.0).into_evm_amount_usdc().unwrap() {
            tracing::error!("Whale account doesn't have enough USDC. Make sure anvil is forked from Arbitrum with: anvil --fork-url https://arb1.lava.build");
            return;
        }

        // Impersonate the whale account
        let impersonate_cmd = serde_json::json!(
            {
                "jsonrpc": "2.0",
                "method": "anvil_impersonateAccount",
                "params": [whale_address],
                "id": 1
            }
        );

        let client = reqwest::Client::new();
        let response = client
            .post(&rpc_url)
            .json(&impersonate_cmd)
            .send()
            .await
            .expect("Failed to send impersonate request");

        if !response.status().is_success() {
            tracing::error!("Failed to impersonate whale account");
            return;
        }

        // Transfer 100 USDC from whale to admin
        let transfer_amount = dec!(100.0).into_evm_amount_usdc().unwrap(); // 100 USDC

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
            "Admin funded with {} USDC",
            admin_balance_after.into_amount_usdc().unwrap()
        );
    } else {
        tracing::info!("Admin already has sufficient USDC balance");
    }

    // Setup connector
    let mut connector = EvmConnector::new();
    connector.start().expect("Failed to start EvmConnector");
    connector
        .connect_arbitrum()
        .await
        .expect("Failed to connect to Arbitrum");
    connector
        .connect_base()
        .await
        .expect("Failed to connect to Base");
    tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;

    // Create designations with admin address
    let source = Arc::new(ComponentLock::new(EvmCollateralDesignation::arbitrum_usdc(
        admin_address,
    )));

    let destination = Arc::new(ComponentLock::new(EvmCollateralDesignation::base_usdc(
        admin_address,
    )));

    // Create bridge using the generic method (it will automatically select Across bridge for cross-chain)
    let bridge = connector.create_bridge(source, destination);

    let chain_id = config
        .get_chain_id("arbitrum")
        .unwrap();
    let client_order_id = "C01".into();
    let route_from = "ARBITRUM".into();
    let route_to = "BASE".into();
    let amount = Amount::try_from("10.00").expect("Invalid amount"); // 10 USDC
    let cumulative_fee = Amount::try_from("0.00").expect("Invalid cumulative fee");

    let (end_tx, mut end_rx) = watch::channel(false);

    bridge
        .write()
        .unwrap()
        .set_observer_fn(move |event: CollateralRouterEvent| match event {
            CollateralRouterEvent::HopComplete {
                chain_id: _,
                address: _,
                client_order_id: _,
                timestamp: _,
                source: _,
                destination,
                route_from: _,
                route_to: _,
                amount,
                fee,
            } => {
                tracing::info!(
                    "Bridge complete: {} USDC -> {} (fee: {})",
                    amount,
                    destination,
                    fee,
                );
                end_tx.send(true).expect("Failed to send ok");
            }
        });

    tracing::info!("Initiating cross-chain transfer: 10 USDC Arbitrum -> Base");

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
    connector.stop().await.expect("Failed to stop EvmConnector");
    tracing::info!("Cross-chain bridge completed");
}
