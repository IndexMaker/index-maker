use alloy::{
    primitives::{address, Address, U256},
    providers::{ext::AnvilApi, Provider, ProviderBuilder},
};
use alloy_chain_connector::util::amount_converter::AmountConverter;
use eyre::{eyre, Context, Result};
use otc_custody::contracts::ERC20;
use rust_decimal::dec;
use std::{
    env,
    process::{Command, Stdio},
    str::FromStr,
    time::Duration,
};
use symm_core::{core::logging::log_init, init_log};
use tokio::time::sleep;
use tracing::{error, info};

fn get_address_from_env(key: &str) -> Result<Address> {
    let s = env::var(key).with_context(|| format!("missing env {key}"))?;
    Address::from_str(s.trim()).with_context(|| format!("invalid {key}"))
}

#[tokio::main]
async fn main() -> Result<()> {
    init_log!();

    tracing::info!("--==| Anvil Provisioner |==--");

    let fork_url =
        env::var("BASE_FORK_URL").unwrap_or_else(|_| String::from("https://mainnet.base.org"));

    let anvil_rpc_url =
        env::var("ANVIL_HTTP").unwrap_or_else(|_| String::from("http://127.0.0.1:8545"));

    let custody_address = get_address_from_env("OTC_CUSTODY_ADDR")
        .unwrap_or_else(|_| address!("0x0A16Da33cf7AC9e6cb7dff65f7755D766ACDB6FE"));

    let usdc_address = get_address_from_env("USDC_ADDR")
        .unwrap_or_else(|_| address!("0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913"));

    let whale_address = get_address_from_env("WHALE_ADDR")
        .unwrap_or_else(|_| address!("0x122fdd9fecbc82f7d4237c0549a5057e31c8ef8d"));

    let fork_block = env::var("FORK_BLOCK_NUMBER").unwrap_or_else(|_| String::from("32638184"));

    info!("Spawning anvil fork at block #{fork_block} …");

    let child = Command::new("anvil")
        .arg("--fork-url")
        .arg(&fork_url)
        .arg("--fork-block-number")
        .arg(fork_block)
        .arg("--chain-id")
        .arg("8453")
        .arg("--auto-impersonate")
        .arg("--block-time")
        .arg("1")
        .arg("--port")
        .arg(anvil_rpc_url.split(':').last().unwrap_or("8545"))
        //.stdout(Stdio::null())
        //.stderr(Stdio::null())
        .spawn()
        .context("failed to spawn anvil")?;

    wait_for_rpc(&anvil_rpc_url).await?;
    info!("Anvil fork online at {}", anvil_rpc_url);

    let provider = ProviderBuilder::new().connect(&anvil_rpc_url).await?;
    let eth_converter = AmountConverter::new(18);
    let usdc_contract = ERC20::new(usdc_address, &provider);
    let usdc_decimals = usdc_contract.decimals().call().await?;
    let usdc_converter = AmountConverter::new(usdc_decimals);

    provider.anvil_auto_impersonate_account(true).await?;

    //
    // Fund Anvil accounts
    //
    provider.anvil_impersonate_account(custody_address).await?;

    provider
        .anvil_set_balance(custody_address, eth_converter.from_amount_safe(dec!(10.0)))
        .await?;

    provider
        .anvil_stop_impersonating_account(custody_address)
        .await?;

    provider.anvil_impersonate_account(whale_address).await?;
    provider
        .anvil_set_balance(
            whale_address,
            eth_converter.from_amount_safe(dec!(10_000.0)),
        )
        .await?;

    let recipients = [
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

    let amount_units = usdc_converter.from_amount_safe(dec!(10_000.0));

    for &to in &recipients {
        let pending = usdc_contract
            .transfer(to, amount_units)
            .from(whale_address)
            .gas(120_000u64)
            .send()
            .await?;
        let receipt = pending.get_receipt().await?;
        if !receipt.status() {
            error!("USDC transfer to {to:#x} failed: {:?}", receipt);
            eyre::bail!("USDC transfer failed");
        }
        info!("10_000 USDC -> {to:#x} | tx {:?}", receipt.transaction_hash);
    }

    provider
        .anvil_stop_impersonating_account(whale_address)
        .await?;

    info!("✅ Anvil ready ok");

    let output = child.wait_with_output()?;

    print!("{}", String::from_utf8(output.stdout).unwrap());
    eprintln!("Stderr: {}", String::from_utf8(output.stderr).unwrap());

    if !output.status.success() {
        eprintln!("Command failed with status: {}", output.status);
    }

    Ok(())
}

async fn wait_for_rpc(rpc_url: &str) -> Result<()> {
    let client = reqwest::Client::new();
    for _ in 0..60 {
        let res = client
            .post(rpc_url)
            .json(&serde_json::json!({"jsonrpc":"2.0","id":1,"method":"eth_chainId","params":[]}))
            .send()
            .await;
        if res.is_ok() {
            return Ok(());
        }
        sleep(Duration::from_millis(500)).await;
    }
    Err(eyre::eyre!("Timed out waiting for Anvil at {rpc_url}"))
}
