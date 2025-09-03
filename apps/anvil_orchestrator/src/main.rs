use alloy_chain_connector::util::amount_converter::AmountConverter;
use eyre::{eyre, Context, Result};
use otc_custody::contracts::ERC20;
use serde_json::json;
use std::{
    process::{Command, Stdio},
    time::Duration,
};
use tokio::time::sleep;
use tracing::{error, info};
use tracing_subscriber::fmt::Subscriber;

use alloy::{
    network::{EthereumWallet, TransactionBuilder},
    primitives::{address, Address, Bytes, U256},
    providers::{Provider, ProviderBuilder},
    rpc::types::TransactionRequest,
    signers::local::PrivateKeySigner,
    sol_types::SolCall,
};

async fn temp_foo() -> Result<()> {
    let provider = ProviderBuilder::new()
        .connect("http://127.0.0.1:8545")
        .await
        .unwrap();

    let usdc_address = address!("0xaf88d065e77c8cC2239327C5EDb3A432268e5831");
    let usdc_contract = ERC20::new(usdc_address, provider);

    let decimals = usdc_contract.decimals().call().await.unwrap();

    println!("USDC DECIMALS: {}", decimals);

    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    Subscriber::builder().with_target(false).init();

    // ----------------- Config (env-overridable) -----------------
    let fork_url =
        std::env::var("FORK_URL").unwrap_or_else(|_| "https://arb1.lava.build".to_string());
    let rpc_url = std::env::var("RPC_URL").unwrap_or_else(|_| "http://127.0.0.1:8545".to_string());
    let chain_id = std::env::var("CHAIN_ID")
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(42161);

    // anvil account(0)
    let admin_pk = std::env::var("ADMIN_PK").unwrap_or_else(|_| {
        "0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80".to_string()
    });
    let admin_addr = address!("0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266");

    // used USDC.e token instead of Native USDC - "0xFF970A61A04b1cA14834A43f5dE4533eBDDB5CC8"
    let usdc_address = std::env::var("USDC_ADDRESS")
        .unwrap_or_else(|_| "0xAf88d065E77c8cC2239327C5EDb3A432268e5831".to_string());
    let usdc: Address = usdc_address.parse().expect("Invalid USDC address");

    let whale_address = std::env::var("WHALE")
        .unwrap_or_else(|_| "0x9dfb9014e88087fba78cc9309c64031d02be9a33".to_string());
    let whale: Address = whale_address.parse().expect("Invalid whale address");

    // will fund 10 USDC as a test
    let fund_amount_human = std::env::var("FUND_AMOUNT").unwrap_or_else(|_| "10.0".to_string());
    let fund_decimals: u32 = 6;

    let recipients: Vec<Address> = vec![
        address!("0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266"),
        address!("0x70997970C51812dc3A010C7d01b50e0d17dc79C8"),
        address!("0x3C44CdDdB6a900fa2b585dd299e03d12FA4293BC"),
        address!("0x90F79bf6EB2c4f870365E785982E1f101E93b906"),
        address!("0x15d34AAf54267DB7D7c367839AAf71A00a2C6A65"),
        address!("0x9965507D1a55bcC2695C58ba16FB37d819B0A4dc"),
        address!("0x976EA74026E726554dB657fA54763abd0C3a0aa9"),
        address!("0x14dC79964da2C08b23698B3D3cc7Ca32193d9955"),
        address!("0x23618e81E3f5cdF7f54C3d65f7FBc0aBf5B21E8f"),
        address!("0xa0Ee7A142d267C1f36714E4a8F75612F20a79720"),
    ];

    // ----------------- 1) Start Anvil (forked) -----------------
    info!("Starting anvil (fork: {fork_url}, chain_id: {chain_id}) …");
    let mut anvil = Command::new("anvil")
        .arg("--fork-url")
        .arg(&fork_url)
        .arg("--chain-id")
        .arg(chain_id.to_string())
        .arg("--auto-impersonate")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .context("Failed to spawn anvil")?;

    // Wait for RPC to be up
    wait_for_rpc(&rpc_url).await?;
    
    temp_foo().await.unwrap();


    // ----------------- 2) Build contract with forge -------------
    info!("Building DepositEmitter with forge …");
    run_cmd("forge", &["build", "--root", "apps/anvil_orchestrator"])?;

    // ----------------- 3) Deploy contract (using bytecode) ------
    let bytecode =
        read_bytecode("apps/anvil_orchestrator/out/DepositEmitter.sol/DepositEmitter.json")?;

    let emitter_address = deploy_contract(&rpc_url, &admin_pk, admin_addr, bytecode).await?;
    info!("DepositEmitter deployed at: {emitter_address:#x}");

    // ----------------- 4) Fund USDC to 10 accounts --------------
    let amount_units = parse_units(&fund_amount_human, fund_decimals)?;
    fund_usdc_to_accounts(&rpc_url, whale, usdc, &recipients, amount_units).await?;

    info!("✅ Setup complete. Anvil is running at {rpc_url}.");

    let provider = ProviderBuilder::new().connect(&rpc_url).await?;
    let usdc_contract = ERC20::new(usdc, &provider);
    let bal = usdc_contract.balanceOf(recipients[0]).call().await?;
    println!("acct0 USDC: {} (raw)", bal);
    println!("acct0 USDC: {} (USDC)", bal / U256::from(1_000_000u64));

    info!("Press Ctrl+C to stop.");

    tokio::signal::ctrl_c().await.ok();
    let _ = anvil.kill();
    Ok(())
}

async fn top_up_eth(rpc_url: &str, addr: Address, eth: u64) -> eyre::Result<()> {
    // 1 ETH = 10^18 wei
    let wei_per_eth: U256 = U256::from(1_000_000_000_000_000_000u128);
    let wei: U256 = U256::from(eth) * wei_per_eth;

    reqwest::Client::new()
        .post(rpc_url)
        .json(&json!({
            "jsonrpc":"2.0","id":1,"method":"anvil_setBalance",
            "params":[format!("{:#x}", addr), format!("0x{:x}", wei)]
        }))
        .send()
        .await?
        .error_for_status()?;
    Ok(())
}

async fn wait_for_rpc(rpc: &str) -> Result<()> {
    let client = reqwest::Client::new();
    for _ in 0..60 {
        let res = client
            .post(rpc)
            .json(&json!({"jsonrpc":"2.0","id":1,"method":"eth_chainId","params":[]}))
            .send()
            .await;
        if res.is_ok() {
            info!("Anvil JSON-RPC is up.");
            return Ok(());
        }
        sleep(Duration::from_millis(500)).await;
    }
    Err(eyre!("Timed out waiting for Anvil JSON-RPC at {rpc}"))
}

fn run_cmd(bin: &str, args: &[&str]) -> Result<()> {
    let status = Command::new(bin)
        .args(args)
        .status()
        .with_context(|| format!("Failed to run {bin}"))?;
    if !status.success() {
        return Err(eyre!("{bin} {:?} exited with {status}", args));
    }
    Ok(())
}

fn read_bytecode(artifact_path: &str) -> Result<Vec<u8>> {
    let text = std::fs::read_to_string(artifact_path)?;
    let v: serde_json::Value = serde_json::from_str(&text)?;
    let obj = v["bytecode"]["object"]
        .as_str()
        .ok_or_else(|| eyre!("bytecode.object missing in artifact"))?;
    let obj_clean = obj.trim_start_matches("0x");
    Ok(hex::decode(obj_clean)?)
}

async fn deploy_contract(
    rpc_url: &str,
    admin_pk: &str,
    admin_addr: Address,
    bytecode: Vec<u8>,
) -> Result<Address> {
    // signer + provider
    let signer: PrivateKeySigner = admin_pk.parse()?;
    let wallet = EthereumWallet::from(signer);
    let provider = ProviderBuilder::new()
        .wallet(wallet)
        .connect(rpc_url)
        .await?;

    let tx = TransactionRequest::default()
        .from(admin_addr)
        .with_deploy_code(bytecode)
        .with_gas_limit(3_000_000u64);

    let pending = provider.send_transaction(tx).await?;
    let receipt = pending.get_receipt().await?;
    let deployed = receipt
        .contract_address
        .ok_or_else(|| eyre!("No contract address in receipt"))?;
    Ok(deployed)
}

async fn fund_usdc_to_accounts(
    rpc_url: &str,
    whale: Address,
    usdc: Address,
    recipients: &[Address],
    amount: U256,
) -> Result<()> {
    // 1) impersonate
    let client = reqwest::Client::new();
    let client = reqwest::Client::new();
    let _ = client
        .post(rpc_url)
        .json(&json!({
            "jsonrpc":"2.0",
            "method":"anvil_impersonateAccount",
            "params":[format!("{:#x}", whale)],
            "id":1
        }))
        .send()
        .await?;

    // 1b) top up whale with ETH for gas
    top_up_eth(rpc_url, whale, 10).await?;

    // 2) provider WITHOUT wallet (node accepts txs from impersonated addr)
    let provider = ProviderBuilder::new().connect(rpc_url).await?;

    println!("RPC URL: {}", rpc_url);
    println!("Whale address: {}", whale);
    println!("USDC contract address: {}", usdc);
    let usdc_contract_whale = ERC20::new(usdc, &provider);
    let usdc_converter = AmountConverter::new(usdc_contract_whale.decimals().call().await?);

    let whale_balance = usdc_contract_whale.balanceOf(whale).call().await?;
    let whale_balance_usdc = usdc_converter.into_amount(whale_balance)?;
    let amount_usdc = usdc_converter.into_amount(amount)?;

    tracing::info!(
        "Whale USDC balance: {} USDC, Amount to transfer: {} USDC, Whale address: {}, USDC address: {}",
        whale_balance_usdc,
        amount_usdc,
        whale,
        usdc,
    );

    if whale_balance < amount {
        panic!("Whale does not have enough balance!");
    }

    // 3) transfer USDC for each recipient
    for &to in recipients {
        let call = ERC20::transferCall { to, amount }.abi_encode();
        let input: Bytes = Bytes::from(call);

        let tx = TransactionRequest::default()
            .with_from(whale)
            .with_to(usdc)
            .with_input(input) // <-- Bytes
            .with_gas_limit(120_000u64);

        let pending = provider.send_transaction(tx).await?;
        let receipt = pending.get_receipt().await?;
        if !receipt.status() {
            error!("USDC transfer to {to:#x} failed: {:?}", receipt);
            return Err(eyre!("USDC transfer failed"));
        }
        info!(
            "{amount} USDC -> {to:#x} | tx {:?}",
            receipt.transaction_hash
        );
    }

    let _ = client
        .post(rpc_url)
        .json(&json!({
            "jsonrpc":"2.0",
            "method":"anvil_stopImpersonatingAccount",
            "params":[format!("{:#x}", whale)],
            "id":1
        }))
        .send()
        .await?;

    Ok(())
}

/// Parse human units (e.g., "10.5") into integer U256 with `decimals`.
fn parse_units(amount: &str, decimals: u32) -> Result<U256> {
    let s = amount.trim();
    if s.is_empty() {
        return Err(eyre!("empty amount"));
    }
    if s.starts_with('-') {
        return Err(eyre!("amount must be positive"));
    }
    let mut parts = s.split('.');
    let int_part = parts.next().unwrap_or("0");
    let frac_part = parts.next().unwrap_or("");
    if parts.next().is_some() {
        return Err(eyre!("invalid number (multiple '.')"));
    }
    if !int_part.chars().all(|c| c.is_ascii_digit()) {
        return Err(eyre!("invalid integer part"));
    }
    if !frac_part.chars().all(|c| c.is_ascii_digit()) {
        return Err(eyre!("invalid fractional part"));
    }
    if frac_part.len() > decimals as usize {
        return Err(eyre!("too many decimal places (max {decimals})"));
    }

    // strip leading zeros in integer part
    let int_norm = int_part.trim_start_matches('0');
    let int_norm = if int_norm.is_empty() { "0" } else { int_norm };

    // right-pad fractional part to the target decimals
    let mut frac_norm = frac_part.to_string();
    frac_norm.extend(std::iter::repeat('0').take(decimals as usize - frac_norm.len()));

    let combined = if decimals == 0 {
        int_norm.to_string()
    } else {
        format!("{int_norm}{frac_norm}")
    };
    let combined = if combined.is_empty() {
        "0".to_string()
    } else {
        combined
    };

    let value: U256 = combined.parse().map_err(|e| eyre!("parse U256: {e:?}"))?;
    Ok(value)
}
