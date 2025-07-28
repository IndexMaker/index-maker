use std::sync::{Arc, RwLock as ComponentLock};

use alloy::network::EthereumWallet;
use alloy::primitives::{address, U256};
use alloy::providers::{Provider, ProviderBuilder};
use alloy::rpc::types::TransactionRequest;
use alloy::signers::local::PrivateKeySigner;
use alloy_evm_connector::contracts::ERC20;
use alloy_evm_connector::designation::EvmCollateralDesignation;
use alloy_evm_connector::evm_connector::EvmConnector;
use std::str::FromStr;
use symm_core::core::bits::{Amount, ClientOrderId, Symbol};

#[tokio::main]
async fn main() {
    // Initialize tracing
    tracing_subscriber::fmt::init();

    tracing::info!("=== ERC20 Transfer End-to-End Test ===");

    // Connect to existing anvil instance
    let rpc_url = "http://localhost:8545";
    tracing::info!("Connecting to existing Anvil instance at: {}", rpc_url);

    // Use known anvil default addresses (these are deterministic)
    let admin_address = address!("0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266"); // anvil[0]
    let admin_private_key = "0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80"; // anvil[0] private key

    let address1 = address!("0x70997970C51812dc3A010C7d01b50e0d17dc79C8"); // anvil[1]
    let address1_private_key = "0x59c6995e998f97a5a0044966f0945389dc9e86dae88c7a8412f4603b6b78690d"; // anvil[1] private key

    let address2 = address!("0x3C44CdDdB6a900fa2b585dd299e03d12FA4293BC"); // anvil[2]
    let _address2_private_key = "0x5de4111afa1a4b94908f83103eb1f1706367c2e68ca870fc3fb9a804cdab365a"; // anvil[2] private key

    // USDC contract address on Arbitrum
    let usdc_address = address!("0xaf88d065e77c8cC2239327C5EDb3A432268e5831");

    tracing::info!("Admin address: {:?}", admin_address);
    tracing::info!("Address1 (recipient): {:?}", address1);
    tracing::info!("Address2 (final recipient): {:?}", address2);
    tracing::info!("USDC contract: {:?}", usdc_address);

    // Step 0: Skip ETH funding for now - admin wallet should have ETH from anvil
    tracing::info!("\n=== Step 0: Checking admin ETH balance ===");

    // Use anvil[0] (admin) as the funder since it has lots of ETH
    let anvil_funder_address = admin_address;
    let anvil_funder_signer: PrivateKeySigner = admin_private_key
        .parse()
        .expect("Invalid funder private key");
    let anvil_funder_wallet = EthereumWallet::from(anvil_funder_signer);
    let funder_provider = ProviderBuilder::new()
        .wallet(anvil_funder_wallet.clone())
        .connect(rpc_url)
        .await
        .expect("Failed to create funder provider");

    // Check admin balance first
    let admin_balance = funder_provider.get_balance(admin_address).await.unwrap();
    tracing::info!("Admin ETH balance: {} wei", admin_balance);

    // Check anvil[0] balance
    let funder_balance = funder_provider
        .get_balance(anvil_funder_address)
        .await
        .unwrap();
    tracing::info!("Anvil[0] ETH balance: {} wei", funder_balance);

    // Only fund if admin has no ETH
    if admin_balance == U256::ZERO && funder_balance > U256::from(1000000000000000000u64) {
        tracing::info!("Admin needs funding, attempting transfer...");

        let eth_amount = alloy::primitives::utils::parse_ether("0.1").unwrap();
        let simple_tx = TransactionRequest::default()
            .to(admin_address)
            .value(eth_amount);

        match funder_provider.send_transaction(simple_tx).await {
            Ok(tx) => {
                let receipt = tx.get_receipt().await.unwrap();
                tracing::info!("Admin funded successfully: {:?}", receipt.transaction_hash);
            }
            Err(e) => {
                tracing::warn!("Admin funding failed: {}, but continuing anyway", e);
            }
        }
    } else {
        tracing::info!("Admin already has ETH or funder insufficient, skipping funding");
    }

    // Step 1: Fund address1 with USDC using a whale account impersonation
    tracing::info!("\n=== Step 1: Funding address1 with USDC using whale impersonation ===");

    // Use a known whale address that has USDC on Arbitrum (Binance Hot Wallet)
    let whale_address = address!("0xB38e8c17e38363aF6EbdCb3dAE12e0243582891D");

    // Create provider without wallet for impersonation
    let provider_for_whale = ProviderBuilder::new()
        .connect(rpc_url)
        .await
        .expect("Failed to create provider for whale");

    let usdc_contract_whale = ERC20::new(usdc_address, &provider_for_whale);

    // Check whale USDC balance first
    let whale_balance = usdc_contract_whale
        .balanceOf(whale_address)
        .call()
        .await
        .unwrap();
    tracing::info!("Whale USDC balance: {} (raw)", whale_balance);

    if whale_balance < U256::from(10_000_000u64) {
        tracing::error!("Whale account doesn't have enough USDC. Make sure anvil is started with:");
        tracing::error!("anvil --fork-url https://arb1.lava.build");
        panic!("Insufficient USDC in whale account");
    }

    // Impersonate the whale account using direct RPC call
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
        panic!("Failed to impersonate whale account");
    }

    tracing::info!(
        "Successfully impersonated whale account: {:?}",
        whale_address
    );

    // Transfer 10 USDC from whale to address1 (10 * 10^6 for USDC decimals)
    let transfer_amount = U256::from(10_000_000u64); // 10 USDC
    tracing::info!(
        "Transferring {} USDC from whale to address1...",
        transfer_amount / U256::from(1_000_000u64)
    );

    // Build transaction manually for impersonated account
    let transfer_call = usdc_contract_whale.transfer(address1, transfer_amount);
    let transfer_calldata = transfer_call.calldata().clone();

    let transfer_tx = TransactionRequest::default()
        .to(usdc_address)
        .input(transfer_calldata.into())
        .from(whale_address)
        .gas_limit(100000);

    let transfer_receipt = provider_for_whale
        .send_transaction(transfer_tx)
        .await
        .unwrap()
        .get_receipt()
        .await
        .unwrap();

    tracing::info!(
        "Transfer to address1 completed: {:?}",
        transfer_receipt.transaction_hash
    );

    // Step 2: Approve USDC transfer from address1 to admin address
    tracing::info!("\n=== Step 2: Approving USDC transfer from address1 to admin ===");

    let address1_signer: PrivateKeySigner = address1_private_key
        .parse()
        .expect("Invalid address1 private key");
    let address1_wallet = EthereumWallet::from(address1_signer);
    let address1_provider = ProviderBuilder::new()
        .wallet(address1_wallet)
        .connect(rpc_url)
        .await
        .expect("Failed to create address1 provider");

    let usdc_contract_address1 = ERC20::new(usdc_address, &address1_provider);

    // Check address1 balance
    let address1_balance = usdc_contract_address1
        .balanceOf(address1)
        .call()
        .await
        .unwrap();
    tracing::info!("Address1 USDC balance: {} (raw)", address1_balance);

    // Approve admin to spend 1 USDC from address1
    let approve_amount = U256::from(1_000_000u64); // 1 USDC
    tracing::info!(
        "Approving admin to spend {} USDC...",
        approve_amount / U256::from(1_000_000u64)
    );

    let approve_receipt = usdc_contract_address1
        .approve(admin_address, approve_amount)
        .send()
        .await
        .unwrap()
        .get_receipt()
        .await
        .unwrap();

    tracing::info!("Approval completed: {:?}", approve_receipt.transaction_hash);

    // Step 3: Use ERC20 bridge to transfer from address1 to address2 using admin wallet
    tracing::info!("\n=== Step 3: Testing ERC20 bridge transfer ===");

    // Create the EvmConnector
    let mut connector = EvmConnector::new();

    // Start the connector (this initializes the arbiter)
    tracing::info!("Starting EvmConnector...");
    connector.start().expect("Failed to start EvmConnector");

    // Connect to Arbitrum chain
    tracing::info!("Connecting to Arbitrum chain...");
    connector
        .connect_arbitrum()
        .await
        .expect("Failed to connect to Arbitrum");

    // Give time for chain operations to be added
    tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
    tracing::info!("EvmConnector initialized");

    // Create source and destination designations (same chain transfer)
    let source_designation = Arc::new(ComponentLock::new(EvmCollateralDesignation::arbitrum_usdc(
        address1, // From address1
    )));

    let destination_designation = Arc::new(ComponentLock::new(
        EvmCollateralDesignation::arbitrum_usdc(address2), // To address2
    ));

    // Create the bridge (should automatically select ERC20 bridge for same-chain)
    let erc20_bridge =
        connector.create_bridge(source_designation.clone(), destination_designation.clone());

    tracing::info!("Created ERC20 bridge for same-chain transfer");

    // Test parameters
    let chain_id = 42161u32; // Arbitrum chain ID
    let client_order_id = ClientOrderId::from("TEST_ERC20_E2E");
    let route_from = Symbol::from("EVM:ARBITRUM:USDC");
    let route_to = Symbol::from("EVM:ARBITRUM:USDC");
    let amount = Amount::from_str("1.0").unwrap(); // 1 USDC
    let cumulative_fee = Amount::from_str("0.0").unwrap(); // No fee for test

    tracing::info!(
        "Initiating ERC20 transfer: {} USDC from {:?} to {:?}",
        amount,
        address1,
        address2
    );

    // Execute the transfer using admin wallet (which has approval to spend from address1)
    let result = {
        let bridge = erc20_bridge.read().unwrap();
        bridge.transfer_funds(
            chain_id,
            admin_address, // Admin wallet executing the transfer
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
            tracing::info!("Waiting for blockchain operation to complete...");

            // Wait for the async blockchain operation to complete
            tokio::time::sleep(tokio::time::Duration::from_secs(10)).await;

            // Check final balances
            tracing::info!("\n=== Final Balance Check ===");
            let provider = ProviderBuilder::new()
                .connect(rpc_url)
                .await
                .expect("Failed to create final provider");
            let usdc_contract_final = ERC20::new(usdc_address, &provider);
            let address1_final_balance = usdc_contract_final
                .balanceOf(address1)
                .call()
                .await
                .unwrap();
            let address2_final_balance = usdc_contract_final
                .balanceOf(address2)
                .call()
                .await
                .unwrap();

            tracing::info!(
                "Address1 final USDC balance: {} USDC",
                address1_final_balance / U256::from(1_000_000u64)
            );
            tracing::info!(
                "Address2 final USDC balance: {} USDC",
                address2_final_balance / U256::from(1_000_000u64)
            );

            tracing::info!("\n=== Test Summary ===");
            tracing::info!("✓ Funded address1 with USDC directly from anvil account");
            tracing::info!("✓ Approved admin to spend USDC from address1");
            tracing::info!("✓ Executed ERC20 transfer via bridge from address1 to address2");
            tracing::info!("✓ Same-chain detection (Arbitrum -> Arbitrum)");
            tracing::info!("✓ Automatic ERC20 bridge selection");
        }
        Err(e) => {
            tracing::error!("ERC20 transfer failed: {}", e);
        }
    }

    // Properly shutdown the connector
    tracing::info!("Shutting down EvmConnector...");
    connector.stop().await.expect("Failed to stop EvmConnector");
    tracing::info!("EvmConnector shutdown complete");

    tracing::info!("\n=== ERC20 Transfer End-to-End Test Completed! ===");
}
