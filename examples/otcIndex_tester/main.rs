use alloy::{
    primitives::{address, Address, Bytes, U256},
    providers::{ext::AnvilApi, DynProvider, Provider, ProviderBuilder},
    rpc::types::{BlockId, BlockNumberOrTag, Filter, FilterBlockOption},
};
use alloy_chain_connector::util::amount_converter::AmountConverter;
use dotenvy::dotenv;
use eyre::{eyre, Context, Result};
use otc_custody::{
    contracts::{IOTCIndex::Deposit, OTCCustody, ERC20},
    custody_authority::CustodyAuthority,
    custody_client::CustodyClientMethods,
    index::{
        index::IndexInstance, index_deployment::IndexDeployment, index_helper::IndexDeployData,
    },
};
use rust_decimal::dec;
use std::{
    env,
    process::{Command, Stdio},
    str::FromStr,
    time::Duration,
};
use tokio::time::sleep;
use tracing::{error, info};
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<()> {
    dotenv().ok();
    let filter = EnvFilter::from_default_env()
        .add_directive("otcIndex_tester=info".parse().unwrap())
        .add_directive("alloy_provider=warn".parse().unwrap());
    tracing_subscriber::fmt().with_env_filter(filter).init();

    let fork_url = env::var("BASE_FORK_URL").context("Please set BASE_FORK_URL")?;
    let custody_address: Address = env_addr("OTC_CUSTODY_ADDR")?;
    let anvil_http = env::var("ANVIL_HTTP").unwrap_or_else(|_| "http://127.0.0.1:8545".to_string());
    let poll_ms: u64 = env::var("POLL_MS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(1000);

    let fork_block = if let Ok(fork_block) = env::var("FORK_BLOCK_NUMBER") {
        fork_block
    } else {
        info!("Connecting to live: {}", fork_url);

        let index_addr: Address = env_addr("OTC_INDEX_ADDR")?;
        //// ---- 1) discover deploy blocks on live Base ----
        let live = ProviderBuilder::new().connect(&fork_url).await?;
        let latest = live.get_block_number().await?;
        let idx_block = find_deploy_block(&live, index_addr, latest).await?;
        let cty_block = find_deploy_block(&live, custody_address, latest).await?;
        let fork_block = idx_block.max(cty_block);
        info!(
            "Using fork from Base block #{fork_block} (OTCIndex at #{idx_block}, OTCCustody at #{cty_block})."
        );

        fork_block.to_string()
    };

    // ---- 2) start Anvil fork ----
    info!("Spawning anvil fork at block #{fork_block} …");

    let _ = Command::new("anvil")
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
        .arg(anvil_http.split(':').last().unwrap_or("8545"))
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .context("failed to spawn anvil")?;

    wait_for_rpc(&anvil_http).await?;
    info!("Anvil fork online at {}", anvil_http);

    // ---- 3) provider + impersonation ----
    let provider = ProviderBuilder::new()
        .connect("http://127.0.0.1:8545")
        .await?;

    provider.anvil_auto_impersonate_account(true).await?;

    {
        // gas for custodian on anvil
        provider.anvil_impersonate_account(custody_address).await?;

        let _ = provider
            .anvil_set_balance(
                custody_address,
                u256("10000000000000000000000")?, // 10k ETH
            )
            .await;

        provider
            .anvil_stop_impersonating_account(custody_address)
            .await?;
    }

    let rpc_url = "http://127.0.0.1:8545";
    let bare = ProviderBuilder::new().connect(rpc_url).await?;

    // ---- 4) funding (keep your original pattern) ----
    let usdc_address: Address = "0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913".parse()?; // Base USDC
    let usdc_bare = ERC20::new(usdc_address, &bare);
    let usdc_decimals = usdc_bare.decimals().call().await?;
    let usdc_converter = AmountConverter::new(usdc_decimals);
    let eth_converter = AmountConverter::new(18);

    let whale: Address = env::var("WHALE")
        .expect("Set WHALE env to a Base USDC whale")
        .parse()
        .expect("Invalid WHALE");

    bare.anvil_impersonate_account(whale).await?;
    bare.anvil_set_balance(
        whale,
        eth_converter
            .from_amount(dec!(10_000))
            .map_err(|e| eyre!(e))?,
    )
    .await?;

    let amount_units = usdc_converter
        .from_amount(dec!(10_000))
        .map_err(|e| eyre!(e))?;

    let chain_id: u32 = 8453;
    let index_factory_address: Address = env_addr("INDEX_FACTORY_ADDR")?;
    let operator: Address = env_addr("OPERATOR_ADDR")?;
    let trade_route: Address = env_addr("TRADE_ROUTE")?;
    let withdraw_route: Address = env_addr("WITHDRAW_ROUTE")?;

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

    for &to in &recipients {
        let pending = usdc_bare
            .transfer(to, amount_units)
            .from(whale)
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
    bare.anvil_stop_impersonating_account(whale).await?;

    // === Realistic ACL + Schnorr + deployConnector + callConnector     ===

    let index_operator = CustodyAuthority::new(|| {
        env::var("SCHNORR_SK").unwrap_or_else(|_| {
            String::from("0x1111111111111111111111111111111111111111111111111111111111111111")
        })
    });

    let index_deploy_data = IndexDeployData {
        name: String::from("Factory Index"),
        symbol: String::from("FI"),
        collateral_token: usdc_address,
        collateral_token_precision: U256::from(6u8),
        management_fee: U256::from(100u64),
        performance_fee: U256::from(200u64),
        max_mint_per_block: u256("1000000000000000000000")?, // 1000 * 1e18
        max_redeem_per_block: u256("1000000000000000000000")?,
        vote_threshold: U256::from(51u8),
        vote_period: U256::from(86400u64),
        initial_price: u256("1000000000000000000000")?, // 100 * 1e18
    };

    let index_builder = IndexDeployment::builder_for(
        index_operator.clone(),
        chain_id,
        index_factory_address,
        custody_address,
        trade_route,
        withdraw_route,
    );

    let index_deployment = index_builder.build(index_deploy_data)?;
    let index_instance = index_deployment.deploy_from(&provider, operator).await?;

    info!("✅ deployConnector ok");

    let index_address = *index_instance.get_index_address();
    let custody_id = index_instance.get_custody_id();

    info!(
        "🆕 OTCIndex deployed: contractAddress: {:#x}, custodyId: {:#x}",
        index_address, custody_id
    );

    {
        // auto-whitelist check
        let otc = OTCCustody::new(custody_address, &provider);
        let whitelisted = otc.isConnectorWhitelisted(index_address).call().await?;
        info!("Index auto-whitelisted: {}", whitelisted);

        // ===== custodyToAddress (deposit then withdraw-to-address via Schnorr) =====
        let collateral_amount_units = usdc_converter
            .from_amount(dec!(100))
            .map_err(|e| eyre!(e))?;

        // withdraw path is towards the user's wallet, so this is user's wallet
        let usdc_withdraw = ERC20::new(usdc_address, &provider);

        // Approve USDC to Custody so it can transferFrom(operator)
        let rc_app = usdc_withdraw
            .approve(custody_address, collateral_amount_units)
            .from(withdraw_route)
            .send()
            .await?
            .get_receipt()
            .await?;

        eyre::ensure!(rc_app.status(), "USDC approve failed: {:?}", rc_app);

        // Deposit into custody under this custodyId
        let rc_dep = otc
            .addressToCustody(custody_id, usdc_address, collateral_amount_units)
            .from(withdraw_route)
            .send()
            .await?
            .get_receipt()
            .await?;

        eyre::ensure!(rc_dep.status(), "addressToCustody failed: {:?}", rc_dep);

        let res = index_instance
            .route_collateral_for_trading_from(&[&provider], trade_route, collateral_amount_units)
            .await?;

        eyre::ensure!(res.status(), "custodyToAddress reverted: {:?}", res);
        info!("✅ custodyToAddress ok: {:?}", res.transaction_hash);

        let balance_withdraw = usdc_bare.balanceOf(withdraw_route).call().await?;
        let balance_trade = usdc_bare.balanceOf(trade_route).call().await?;
        info!(
            "USDC balance: Wallet {}: {} ==> Trading Account {}: {}",
            withdraw_route,
            usdc_converter.into_amount(balance_withdraw)?,
            trade_route,
            usdc_converter.into_amount(balance_trade)?
        );
    }

    // // 5) callConnector: curatorUpdate + solverUpdate (empty proof when whitelisted)
    let weights1 = Bytes::from(vec![0u8; 32]);
    let price1 = u256("120000000000000000000")?;

    let res = index_instance
        .set_currator_weights_from(&provider, operator, &weights1, price1)
        .await?;

    eyre::ensure!(res.status(), "curatorUpdate reverted: {:?}", res);
    info!("✅ curatorUpdate ok: {:?}", res.transaction_hash);

    let weights2 = Bytes::from(vec![1u8; 32]);
    let price2 = u256("130000000000000000000")?;

    let res = index_instance
        .solver_weights_set_from(&provider, operator, &weights2, price2)
        .await?;

    eyre::ensure!(res.status(), "solverUpdate reverted: {:?}", res);
    info!("✅ solverUpdate ok: {:?}", res.transaction_hash);

    info!("Waiting deposit event from Client side...");
    let _ = wait_for_deposit_and_mint(index_instance, provider, poll_ms).await?;

    Ok(())
}

// --------------------------- my helpers ---------------------------

fn u256(s: &str) -> eyre::Result<U256> {
    U256::from_str(s).map_err(|e| eyre!(e))
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

async fn find_deploy_block<P: Provider>(p: &P, addr: Address, latest: u64) -> Result<u64> {
    let mut lo = 1u64;
    let mut hi = latest;
    let mut ans = latest;
    while lo <= hi {
        let mid = (lo + hi) / 2;
        let code = p
            .get_code_at(addr)
            .block_id(BlockId::Number(BlockNumberOrTag::Number(mid)))
            .await?;
        if !code.0.is_empty() {
            ans = mid;
            if mid == 0 {
                break;
            }
            hi = mid.saturating_sub(1);
        } else {
            lo = mid + 1;
        }
    }
    Ok(ans)
}

fn env_addr(key: &str) -> Result<Address> {
    let s = env::var(key).with_context(|| format!("missing env {key}"))?;
    Address::from_str(s.trim()).with_context(|| format!("invalid {key}"))
}

/// Wait for the Deposit events and Mint immeidately for capturing on Client.
async fn wait_for_deposit_and_mint(
    index_instance: IndexInstance,
    provider: impl Provider + Clone + 'static,
    poll_millis: u64,
) -> Result<()> {
    let index_address = *index_instance.get_index_address();
    let mut last_block_from = provider.get_block_number().await?;
    let event_filter = Filter::new().address(index_address);
    loop {
        let most_recent_block = provider.get_block_number().await?;
        if most_recent_block > last_block_from {
            let range = event_filter.clone().select(FilterBlockOption::Range {
                from_block: Some(BlockNumberOrTag::Number(last_block_from + 1)),
                to_block: Some(BlockNumberOrTag::Number(most_recent_block)),
            });

            let logs = provider.get_logs(&range).await?;
            for log_event in logs {
                if let Ok(deposit_event) = log_event.log_decode::<Deposit>() {
                    let deposit_data = deposit_event.inner;
                    info!(
                        "📥 Deposit: amount={} from={:#x} seq={} aff1={:#x} aff2={:#x}",
                        deposit_data.amount,
                        deposit_data.from,
                        deposit_data.seqNumNewOrderSingle,
                        deposit_data.affiliate1,
                        deposit_data.affiliate2
                    );

                    let usdc_address = *index_instance.get_collateral_token_address();
                    let usdc_converter =
                        AmountConverter::new(index_instance.get_collateral_token_precision());
                    let index_converter =
                        AmountConverter::new(index_instance.get_index_token_precision());

                    let trade_route = index_instance.get_trade_route();

                    let provider = DynProvider::new(provider.clone());
                    let custody_owner = index_instance.get_custody_owner(&provider).await?;

                    let res = index_instance
                        .route_collateral_to_from(
                            &[&provider],
                            &custody_owner,
                            trade_route,
                            &usdc_address,
                            deposit_data.amount,
                        )
                        .await?;

                    eyre::ensure!(res.status(), "Collateral routing for trading: {:?}", res);

                    let usdc_contract = ERC20::new(usdc_address, &provider);

                    let balance_withdraw =
                        usdc_contract.balanceOf(deposit_data.from).call().await?;
                    let balance_trade = usdc_contract.balanceOf(*trade_route).call().await?;
                    info!(
                        "✅ Collateral routed: USDC balance: Wallet [{}] ${} ==> Trading Account [{}] ${} (tx: {:?})",
                        deposit_data.from,
                        usdc_converter.into_amount(balance_withdraw)?,
                        trade_route,
                        usdc_converter.into_amount(balance_trade)?,
                 res.transaction_hash
                    );

                    // Mint after deposit
                    let index_price = index_instance.get_initial_price();
                    let mint_amount = index_converter
                        .rescale_from(deposit_data.amount, &usdc_converter)
                        / index_price;

                    let rc = index_instance
                        .mint_index_from(
                            &provider,
                            custody_owner,
                            deposit_data.from,
                            mint_amount,
                            deposit_data.seqNumNewOrderSingle,
                        )
                        .await?;

                    eyre::ensure!(rc.status(), "mint via callConnector reverted: {:?}", rc);

                    info!(
                        "🎟️ Minted {} to {:#x} (seq {}) | tx {:?}",
                        mint_amount,
                        deposit_data.from,
                        deposit_data.seqNumNewOrderSingle,
                        rc.transaction_hash
                    );

                    continue;
                }
            }
            last_block_from = most_recent_block;
        }
        tokio::time::sleep(std::time::Duration::from_millis(poll_millis)).await;
    }
}
