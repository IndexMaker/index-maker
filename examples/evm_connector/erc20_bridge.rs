use std::sync::{Arc, RwLock as ComponentLock};

use alloy::network::EthereumWallet;
use alloy::primitives::address;
use alloy::providers::{Provider, ProviderBuilder};
use alloy::rpc::types::TransactionRequest;
use alloy::signers::local::PrivateKeySigner;
use alloy_evm_connector::config::EvmConnectorConfig;
use alloy_evm_connector::contracts::ERC20;
use alloy_evm_connector::designation::EvmCollateralDesignation;
use alloy_evm_connector::evm_connector::EvmConnector;
use alloy_evm_connector::utils::{IntoAmount, IntoEvmAmount};
use rust_decimal::dec;
use symm_core::core::bits::{ClientOrderId, Symbol};

#[tokio::main]
async fn main() {
    // Initialize tracing
    tracing_subscriber::fmt::init();

    tracing::info!("=== ERC20 Transfer Test ===");

    ////////
    //
    // IMPORTANT NOTE
    // ==============
    // To run this example successfully you need to create .env file
    // which must contain PRIVATE_KEY same as `address2_private_key` below.
    //
    // While this isn't how EVM connector & bridges were supposed to be working
    // atm this is what needs to be done to make this example pass.
    //
    // TODO
    // ====
    // We need to change how Alloy EVM Connector works.
    //
    // - EVM Connector should have Arbiter
    //
    // - Each EVM Designation should have its own Private Key, and
    //   its own command polling loop for any operations that require signing
    //   like transfers.
    //
    // - In case of transfer across two newtorks, Across Bridge should initialize
    //   AcrossDepositBuilder, which then can be used from source designation to
    //   destination designation.
    //
    //

    let config = EvmConnectorConfig::default();
    let rpc_url = EvmConnectorConfig::get_default_rpc_url();
    tracing::info!("RPC Url: {}", rpc_url);

    let from_address = address!("0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266");

    let address1 = address!("0x70997970C51812dc3A010C7d01b50e0d17dc79C8");
    let address1_private_key = "0x59c6995e998f97a5a0044966f0945389dc9e86dae88c7a8412f4603b6b78690d";

    let address2 = address!("0x3C44CdDdB6a900fa2b585dd299e03d12FA4293BC");
    let address2_private_key = "0x5de4111afa1a4b94908f83103eb1f1706367c2e68ca870fc3fb9a804cdab365a";

    let address3 = address!("0x90F79bf6EB2c4f870365E785982E1f101E93b906");

    let usdc_address = config.get_usdc_address("arbitrum").unwrap();

    // Fund address1 with USDC (whale impersonation)
    let whale_address = address!("0x9dfb9014e88087fba78cc9309c64031d02be9a33");
    let provider_for_whale = ProviderBuilder::new()
        .connect(&rpc_url)
        .await
        .expect("Failed to create provider");
    let usdc_contract_whale = ERC20::new(usdc_address, &provider_for_whale);

    let whale_balance = usdc_contract_whale
        .balanceOf(whale_address)
        .call()
        .await
        .unwrap();
    let decimals = usdc_contract_whale.decimals().call().await.unwrap();

    tracing::info!(
        "Whale USDC balance: {} ({} USDC), Whale address: {}, USDC address: {}, Decimals: {}",
        whale_balance,
        whale_balance.into_amount_usdc().unwrap(),
        whale_address,
        usdc_address,
        decimals,
    );

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
    let transfer_amount = dec!(10.0).into_evm_amount_usdc().unwrap();
    let transfer_call = usdc_contract_whale.transfer(address1, transfer_amount);
    let transfer_tx = TransactionRequest::default()
        .to(usdc_address)
        .input(transfer_call.calldata().clone().into())
        .from(whale_address)
        .gas_limit(100000);

    provider_for_whale
        .send_transaction(transfer_tx)
        .await
        .unwrap()
        .get_receipt()
        .await
        .unwrap();

    // First some examples of how we can use ERC20 transfer(), vs approve() and transferFrom()
    // ---

    let address1_wallet = EthereumWallet::from(
        address1_private_key
            .parse::<PrivateKeySigner>()
            .expect("Invalid private key"),
    );

    let address1_provider = ProviderBuilder::new()
        .wallet(address1_wallet)
        .connect(&rpc_url)
        .await
        .expect("Failed to create provider");

    let usdc_contract_address1 = ERC20::new(usdc_address, &address1_provider);
    let approve_amount = dec!(1.0).into_evm_amount_usdc().unwrap(); // 1 USDC

    // Here we call transfer() -- this is direct transfer from our wallet to
    // some other wallet in this case address1 is our wallet and address2 is
    // other wallet.
    tracing::info!("Address1 is directly transferring to Address2");
    usdc_contract_address1
        .transfer(address2, approve_amount)
        .send()
        .await
        .unwrap()
        .get_receipt()
        .await
        .unwrap();

    let address2_wallet = EthereumWallet::from(
        address2_private_key
            .parse::<PrivateKeySigner>()
            .expect("Invalid private key"),
    );

    let address2_provider = ProviderBuilder::new()
        .wallet(address2_wallet)
        .connect(&rpc_url)
        .await
        .expect("Failed to create provider");

    // Here we call approve() and then transferFrom() -- we own our wallet, so
    // we approve that owner of the other wallet can withdraw form us given
    // amount.
    let usdc_contract_address2 = ERC20::new(usdc_address, &address2_provider);
    tracing::info!("Address1 is approving Address2 to withdraw");
    usdc_contract_address1
        .approve(address2, approve_amount)
        .send()
        .await
        .unwrap()
        .get_receipt()
        .await
        .unwrap();

    let allowance = usdc_contract_address2
        .allowance(address1, address2)
        .call()
        .await
        .unwrap();
    tracing::info!(%allowance, "Allowance");

    // Now the other party is withdrawing from us, so they use their PK to
    // identify them-selves, and they can withdraw to any addess they like.
    tracing::info!("Address2 is withdrawing from Address1 and sending that to Address3");
    usdc_contract_address2
        .transferFrom(address1, address3, approve_amount)
        .send()
        .await
        .unwrap()
        .get_receipt()
        .await
        .unwrap();

    // We can use public (no-PK) provider to obtain balances
    let public_provider = ProviderBuilder::new()
        .connect(&rpc_url)
        .await
        .expect("Failed to create provider");
    let usdc_contract_public = ERC20::new(usdc_address, &public_provider);

    let balance = usdc_contract_public
        .balanceOf(address1)
        .call()
        .await
        .unwrap();
    tracing::info!(%balance, balance_usd=%balance.into_amount_usdc().unwrap(), "Address1 Balance");

    let balance = usdc_contract_public
        .balanceOf(address2)
        .call()
        .await
        .unwrap();
    tracing::info!(%balance, balance_usd=%balance.into_amount_usdc().unwrap(), "Address2 Balance");

    let balance = usdc_contract_public
        .balanceOf(address3)
        .call()
        .await
        .unwrap();
    tracing::info!(%balance, balance_usd=%balance.into_amount_usdc().unwrap(), "Address3 Balance");

    // Test ERC20 bridge transfer
    tracing::info!("We'll check Erc20 Bridge now...");

    let mut connector = EvmConnector::new();
    connector.start().expect("Failed to start EvmConnector");
    connector
        .connect_arbitrum()
        .await
        .expect("Failed to connect to Arbitrum");
    tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;

    let source_designation = Arc::new(ComponentLock::new(EvmCollateralDesignation::arbitrum_usdc(
        address1,
    )));
    let destination_designation = Arc::new(ComponentLock::new(
        EvmCollateralDesignation::arbitrum_usdc(address2),
    ));
    let erc20_bridge =
        connector.create_bridge(source_designation.clone(), destination_designation.clone());

    let chain_id = config.get_chain_id("arbitrum").unwrap();
    let client_order_id = ClientOrderId::from("TEST_ERC20_E2E");
    let route_from = Symbol::from("EVM:ARBITRUM:USDC");
    let route_to = Symbol::from("EVM:ARBITRUM:USDC");
    let amount = dec!(1.0);
    let cumulative_fee = dec!(0.0);

    // Execute transfer
    {
        // Not ideal, but required to pass this example.
        // We need to remove dependency on approve(), which comes from
        // the use of transferFrom() within ChainOperations. Once we
        // rework EVM Connector so that we have separate polling loop
        // for each EVM Designation with its own PK, then we can use
        // transfer() method, and that will transfer from the wallet
        // represented by that Designation into target wallet.
        usdc_contract_address1
            .approve(address2, approve_amount)
            .send()
            .await
            .unwrap()
            .get_receipt()
            .await
            .unwrap();

        let allowance = usdc_contract_address2
            .allowance(address1, address2)
            .call()
            .await
            .unwrap();
        tracing::info!(%allowance, %address1, %address2, "Allowance");

        let bridge = erc20_bridge.read().unwrap();
        bridge.transfer_funds(
            chain_id,
            from_address,
            client_order_id,
            route_from,
            route_to,
            amount,
            cumulative_fee,
        )
    }
    .expect("Transfer failed");

    tokio::time::sleep(tokio::time::Duration::from_secs(10)).await;
    connector.stop().await.expect("Failed to stop EvmConnector");
    tracing::info!("ERC20 transfer completed");
}
