use std::sync::{Arc, RwLock as ComponentLock};

use alloy::network::EthereumWallet;
use alloy::primitives::{address, U256};
use alloy::providers::{Provider, ProviderBuilder};
use alloy::rpc::types::TransactionRequest;
use alloy::signers::local::PrivateKeySigner;
use alloy_evm_connector::contracts::ERC20;
use alloy_evm_connector::config::EvmConnectorConfig;
use alloy_evm_connector::designation::EvmCollateralDesignation;
use alloy_evm_connector::evm_connector::EvmConnector;
use std::str::FromStr;
use symm_core::core::bits::{Amount, ClientOrderId, Symbol};

#[tokio::main]
async fn main() {
    // Initialize tracing
    tracing_subscriber::fmt::init();

    tracing::info!("=== ERC20 Transfer Test ===");

    let config = EvmConnectorConfig::default();
    let rpc_url = EvmConnectorConfig::get_default_rpc_url();
    let admin_address = address!("0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266"); 
    let admin_private_key = "0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80"; 
    let address1 = address!("0x70997970C51812dc3A010C7d01b50e0d17dc79C8"); 
    let address1_private_key = "0x59c6995e998f97a5a0044966f0945389dc9e86dae88c7a8412f4603b6b78690d"; 
    let address2 = address!("0x3C44CdDdB6a900fa2b585dd299e03d12FA4293BC"); 
    let _address2_private_key = "0x5de4111afa1a4b94908f83103eb1f1706367c2e68ca870fc3fb9a804cdab365a"; 
    let usdc_address = config.get_usdc_address(42161).unwrap();

    // Setup provider
    let anvil_funder_signer: PrivateKeySigner = admin_private_key.parse().expect("Invalid private key");
    let anvil_funder_wallet = EthereumWallet::from(anvil_funder_signer);
    let funder_provider = ProviderBuilder::new()
        .wallet(anvil_funder_wallet.clone())
        .connect(&rpc_url)
        .await
        .expect("Failed to create provider");

    // Fund address1 with USDC (whale impersonation)
    let whale_address = address!("0xB38e8c17e38363aF6EbdCb3dAE12e0243582891D");
    let provider_for_whale = ProviderBuilder::new().connect(&rpc_url).await.expect("Failed to create provider");
    let usdc_contract_whale = ERC20::new(usdc_address, &provider_for_whale);

    // Impersonate whale account
    let impersonate_cmd = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "anvil_impersonateAccount", 
        "params": [whale_address],
        "id": 1
    });
    
    let client = reqwest::Client::new();
    let response = client
        .post(&rpc_url)
        .json(&impersonate_cmd)
        .send()
        .await
        .expect("Failed to send impersonate request");
        
    if !response.status().is_success() {
        panic!("Failed to impersonate whale account");
    }

    // Transfer 10 USDC from whale to address1
    let transfer_amount = U256::from(EvmConnectorConfig::get_default_deposit_amount() * 10);
    let transfer_call = usdc_contract_whale.transfer(address1, transfer_amount);
    let transfer_tx = TransactionRequest::default()
        .to(usdc_address)
        .input(transfer_call.calldata().clone().into())
        .from(whale_address)
        .gas_limit(100000);
    
    provider_for_whale.send_transaction(transfer_tx).await.unwrap().get_receipt().await.unwrap();

    // Approve USDC transfer
    let address1_signer: PrivateKeySigner = address1_private_key.parse().expect("Invalid private key");
    let address1_wallet = EthereumWallet::from(address1_signer);
    let address1_provider = ProviderBuilder::new()
        .wallet(address1_wallet)
        .connect(&rpc_url)
        .await
        .expect("Failed to create provider");

    let usdc_contract_address1 = ERC20::new(usdc_address, &address1_provider);
    let approve_amount = U256::from(EvmConnectorConfig::get_default_deposit_amount()); // 1 USDC
    usdc_contract_address1.approve(admin_address, approve_amount).send().await.unwrap().get_receipt().await.unwrap();

    // Test ERC20 bridge transfer
    let mut connector = EvmConnector::new();
    connector.start().expect("Failed to start EvmConnector");
    connector.connect_arbitrum().await.expect("Failed to connect to Arbitrum");
    tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;

    let source_designation = Arc::new(ComponentLock::new(EvmCollateralDesignation::arbitrum_usdc(address1)));
    let destination_designation = Arc::new(ComponentLock::new(EvmCollateralDesignation::arbitrum_usdc(address2)));
    let erc20_bridge = connector.create_bridge(source_designation.clone(), destination_designation.clone());

    let chain_id = 42161u32;
    let client_order_id = ClientOrderId::from("TEST_ERC20_E2E");
    let route_from = Symbol::from("EVM:ARBITRUM:USDC");
    let route_to = Symbol::from("EVM:ARBITRUM:USDC");
    let amount = Amount::from_str("1.0").unwrap(); 
    let cumulative_fee = Amount::from_str("0.0").unwrap();

    // Execute transfer
    {
        let bridge = erc20_bridge.read().unwrap();
        bridge.transfer_funds(chain_id, admin_address, client_order_id, route_from, route_to, amount, cumulative_fee)
    }.expect("Transfer failed");

    tokio::time::sleep(tokio::time::Duration::from_secs(10)).await;
    connector.stop().await.expect("Failed to stop EvmConnector");
    tracing::info!("ERC20 transfer completed");
}
