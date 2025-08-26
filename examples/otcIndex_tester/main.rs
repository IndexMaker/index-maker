use alloy::{
    primitives::{address, keccak256, Address, Bytes, B256, U256},
    providers::{ext::AnvilApi, Provider, ProviderBuilder},
    rpc::types::{BlockId, BlockNumberOrTag, Filter, FilterBlockOption, TransactionRequest},
    sol,
    sol_types::SolValue,
};
use alloy_chain_connector::util::amount_converter::AmountConverter;
use anyhow::{anyhow, Context, Result};
use ca_helper::contracts::{IndexFactory, OTCCustody, OTCIndex as OTCIndexContract, ERC20, SchnorrCAKey, SchnorrSignature, VerificationData};
use ca_helper::custody_helper::{CAHelper, Party};
use dotenvy::dotenv;
use rust_decimal::dec;
use std::{
    env,
    process::{Command, Stdio},
    str::FromStr,
    time::Duration,
};
use tokio::time::sleep;
use tracing::{error, info, warn};
use tracing_subscriber::EnvFilter;

// ---------- helpers: key → (parity, x) for Schnorr pubkey ----------
fn pubkey_parity_x(sk_hex: &str) -> Result<(u8, B256)> {
    use k256::ecdsa::VerifyingKey as EcdsaVK;

    let sk_bytes = hex::decode(sk_hex.trim_start_matches("0x"))?;
    let signing_key = k256::ecdsa::SigningKey::from_bytes((&sk_bytes).into())?;
    let vk: EcdsaVK = signing_key.verifying_key();

    // compressed: 0x02/0x03 || x
    let ep = vk.to_encoded_point(true);
    let tag = ep.as_bytes()[0];
    let parity = if tag == 0x02 { 0u8 } else { 1u8 };
    let x_bytes = &ep.as_bytes()[1..33];
    Ok((parity, B256::from_slice(x_bytes)))
}

// Deterministic Schnorr-ish helper to mirror on-chain challenge style:
// e = keccak256(message); k = keccak256(sk||message); s = k + e*sk   (mod n)
fn schnorr_sign_keccak_challenge(sk_hex: &str, message: &[u8]) -> Result<(B256, B256)> {
    use k256::{elliptic_curve::ScalarPrimitive, Scalar};

    let sk_be = hex::decode(sk_hex.trim_start_matches("0x"))?;
    let x =
        Scalar::from_repr(*ScalarPrimitive::from_bytes(&sk_be).ok_or_else(|| anyhow!("bad sk"))?)
            .ok_or_else(|| anyhow!("sk out of range"))?;

    let e = B256::from(keccak256(message));
    let e_scalar =
        Scalar::from_repr(*ScalarPrimitive::from_bytes(e.as_slice()).unwrap_or_default())
            .unwrap_or(Scalar::ZERO);

    let mut k_input = Vec::with_capacity(sk_be.len() + message.len());
    k_input.extend_from_slice(&sk_be);
    k_input.extend_from_slice(message);
    let k_hash = keccak256(&k_input);
    let k_scalar =
        Scalar::from_repr(*ScalarPrimitive::from_bytes(k_hash.as_slice()).unwrap_or_default())
            .unwrap_or(Scalar::ONE);

    let s = k_scalar + (e_scalar * x);
    let mut s32 = [0u8; 32];
    s.to_bytes().copy_to_slice(&mut s32);
    Ok((e, B256::from(s32)))
}

// Message domains (match OTCCustody.sol VerificationUtils.verifySchnorr)
fn deploy_connector_message(
    ts: U256,
    id: B256,
    connector_type: &str,
    factory: Address,
    data: &Bytes,
) -> Bytes {
    SolValue::abi_encode_params(&(
        ts,
        "deployConnector".to_string(),
        id,
        connector_type.to_string(),
        factory,
        data.clone(),
    ))
}

fn call_connector_message(
    ts: U256,
    id: B256,
    connector_type: &str,
    connector_addr: Address,
    fixed: &Bytes,
    tail: &Bytes,
) -> Bytes {
    SolValue::abi_encode_params(&(
        ts,
        "callConnector".to_string(),
        id,
        connector_type.to_string(),
        connector_addr,
        fixed.clone(),
        tail.clone(),
    ))
}

#[tokio::main]
async fn main() -> Result<()> {
    dotenv().ok();
    let filter = EnvFilter::from_default_env()
        .add_directive("otcindex_factory_flow=info".parse().unwrap())
        .add_directive("alloy_provider=warn".parse().unwrap());
    tracing_subscriber::fmt().with_env_filter(filter).init();

    // ---- env ----
    let fork_url =
        env::var("BASE_FORK_URL").context("Please set BASE_FORK_URL (archive RPC preferred)")?;
    let index_addr: Address = env_addr("OTC_INDEX_ADDR")?;
    let custody_addr: Address = env_addr("OTC_CUSTODY_ADDR")?;
    let anvil_http = env::var("ANVIL_HTTP").unwrap_or_else(|_| "http://127.0.0.1:8545".to_string());
    let poll_ms: u64 = env::var("POLL_MS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(1000);

    info!("Connecting to live: {}", fork_url);

    // ---- 1) discover deploy blocks on live Base ----
    let live = ProviderBuilder::new().connect(&fork_url).await?;
    let latest = live.get_block_number().await?;
    let idx_block = find_deploy_block(&live, index_addr, latest).await?;
    let cty_block = find_deploy_block(&live, custody_addr, latest).await?;
    let fork_block = idx_block.max(cty_block);
    info!(
        "Using fork from Base block #{fork_block} (OTCIndex at #{idx_block}, OTCCustody at #{cty_block})."
    );

    // ---- 2) start Anvil fork ----
    info!("Spawning anvil fork at block #{fork_block} …");
    let _ = Command::new("anvil")
        .arg("--fork-url")
        .arg(&fork_url)
        .arg("--fork-block-number")
        .arg(fork_block.to_string())
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

    let rpc_url = "http://127.0.0.1:8545";
    let bare = ProviderBuilder::new().connect(rpc_url).await?;

    // ---- 4) funding (keep your original pattern) ----
    let usdc: Address = "0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913".parse()?; // Base USDC
    let usdc_contract = ERC20::new(usdc, &bare);
    let usdc_decimals = usdc_contract.decimals().call().await?;
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
            .map_err(|e| anyhow!(e))?,
    )
    .await?;

    let amount_units = usdc_converter
        .from_amount(dec!(10_000))
        .map_err(|e| anyhow!(e))?;

    let recipients: [Address; 10] = [
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
        let pending = usdc_contract
            .transfer(to, amount_units)
            .from(whale)
            .gas(120_000u64)
            .send()
            .await?;
        let receipt = pending.get_receipt().await?;
        if !receipt.status() {
            error!("USDC transfer to {to:#x} failed: {:?}", receipt);
            anyhow::bail!("USDC transfer failed");
        }
        info!("10_000 USDC -> {to:#x} | tx {:?}", receipt.transaction_hash);
    }
    bare.anvil_stop_impersonating_account(whale).await?;

    // =====================================================================
    // === Realistic ACL + Schnorr + deployConnector + callConnector     ===
    // =====================================================================

    // Factory + signer
    let index_factory_addr: Address = env_addr("INDEX_FACTORY_ADDR")?;
    let schnorr_sk_hex = env::var("SCHNORR_SK").unwrap_or_else(|_| {
        "0x1111111111111111111111111111111111111111111111111111111111111111".into()
    });
    let chain_id: u64 = 8453;

    // 1) Build CA/ACL for deployConnector ("OTCIndex") and compute CustodyID
    let (parity, x) = pubkey_parity_x(&schnorr_sk_hex)?;
    let party = Party { parity, x };
    let custody_state: u8 = 0;

    // index params (mirror your TS script)
    let index_params = (
        "Factory Index".to_string(),
        "FI".to_string(),
        usdc,            // collateral
        U256::from(6u8), // decimals
        U256::from(100u64),
        U256::from(200u64),
        U256::from_dec_str("1000000000000000000000")?, // 1000 * 1e18
        U256::from_dec_str("1000000000000000000000")?,
        U256::from(51u8),
        U256::from(86400u64),
        U256::from_dec_str("100000000000000000000")?, // 100 * 1e18
    );
    let deploy_data: Bytes = SolValue::abi_encode_params(&index_params).into();

    let mut ca = CAHelper::new(chain_id as u32, custody_addr);
    let leaf_idx_deploy = ca.deploy_connector(
        "OTCIndex",
        index_factory_addr,
        deploy_data.as_ref(),
        custody_state,
        party.clone(),
    );
    let custody_id = B256::from_slice(&ca.get_custody_id());
    info!(
        "CustodyID (CA root): 0x{}",
        hex::encode(custody_id.as_slice())
    );

    // 2) Build VerificationData for deployConnector
    let latest_ts: U256 = provider
        .get_block_by_number(BlockNumberOrTag::Latest)
        .await?
        .map(|b| U256::from(b.header().timestamp()))
        .unwrap_or(U256::ZERO);

    let msg = deploy_connector_message(
        latest_ts,
        custody_id,
        "OTCIndex",
        index_factory_addr,
        &deploy_data,
    );
    let (e, s) = schnorr_sign_keccak_challenge(&schnorr_sk_hex, msg.as_ref())?;

    let proof = ca
        .get_merkle_proof(leaf_idx_deploy)
        .into_iter()
        .map(|arr| B256::from_slice(&arr))
        .collect::<Vec<_>>();

    let v_deploy = VerificationData {
        id: custody_id,
        state: custody_state,
        timestamp: latest_ts,
        pubKey: SchnorrCAKey { parity, x },
        sig: SchnorrSignature { e, s },
        merkleProof: proof,
    };

    // 3) Call deployConnector on OTCCustody
    let otc = OTCCustody::new(custody_addr, provider.clone());

    // gas for custodian on anvil
    provider.anvil_impersonate_account(custody_addr).await?;
    let _ = provider
        .anvil_set_balance(
            custody_addr,
            U256::from_dec_str("10000000000000000000000")?, // 10k ETH
        )
        .await;
    provider
        .anvil_stop_impersonating_account(custody_addr)
        .await?;

    let tx = OTCCustody::deployConnectorCall {
        _connectorType: "OTCIndex".to_string(),
        _factoryAddress: index_factory_addr,
        _data: deploy_data.clone(),
        v: v_deploy,
    };
    let rc = otc.deployConnector(tx).send().await?.get_receipt().await?;
    anyhow::ensure!(rc.status(), "deployConnector reverted: {:?}", rc);
    info!("✅ deployConnector ok: {:?}", rc.transaction_hash);

    // 4) Read IndexDeployed to get deployed index
    let factory = IndexFactory::new(index_factory_addr, provider.clone());
    let logs = provider
        .get_logs(&Filter::new().address(index_factory_addr))
        .await?;
    let mut deployed_index = Address::ZERO;
    for lg in logs {
        if let Ok(parsed) = lg.log_decode::<IndexFactory::IndexDeployed>() {
            deployed_index = parsed.inner.indexAddress;
        }
    }
    anyhow::ensure!(
        deployed_index != Address::ZERO,
        "IndexDeployed event not found"
    );

    info!("🆕 OTCIndex deployed at: {deployed_index:#x}");

    // auto-whitelist check
    let whitelisted = otc.isConnectorWhitelisted(deployed_index).call().await?;
    info!("Index auto-whitelisted: {}", whitelisted);

    // 5) callConnector: curatorUpdate + solverUpdate (empty proof when whitelisted)
    let ts1 = latest_ts + U256::from(1);
    let weights1 = Bytes::from(vec![0u8; 32]);
    let price1 = U256::from_dec_str("120000000000000000000")?; // 120 * 1e18
    let curator_call: Bytes = SolValue::abi_encode_params(&(ts1, weights1.clone(), price1)).into();

    let msg_curator = call_connector_message(
        ts1,
        custody_id,
        "OTCIndexConnector",
        deployed_index,
        &curator_call,
        &Bytes::default(),
    );
    let (e2, s2) = schnorr_sign_keccak_challenge(&schnorr_sk_hex, msg_curator.as_ref())?;
    let v_call1 = OTCCustody::VerificationData {
        id: custody_id,
        state: custody_state,
        timestamp: ts1,
        pubKey: SchnorrCAKey { parity, x },
        sig: SchnorrSignature { e: e2, s: s2 },
        merkleProof: vec![],
    };

    let tx2 = OTCCustody::callConnectorCall {
        connectorType: "OTCIndexConnector".to_string(),
        connectorAddress: deployed_index,
        fixedCallData: curator_call.clone(),
        tailCallData: Bytes::default(),
        v: v_call1,
    };
    let rc2 = otc.callConnector(tx2).send().await?.get_receipt().await?;
    anyhow::ensure!(rc2.status(), "curatorUpdate reverted: {:?}", rc2);
    info!("✅ curatorUpdate ok: {:?}", rc2.transaction_hash);

    let ts2 = ts1 + U256::from(1);
    let weights2 = Bytes::from(vec![1u8; 32]);
    let price2 = U256::from_dec_str("130000000000000000000")?; // 130 * 1e18
    let solver_call: Bytes = SolValue::abi_encode_params(&(ts2, weights2.clone(), price2)).into();

    let msg_solver = call_connector_message(
        ts2,
        custody_id,
        "OTCIndexConnector",
        deployed_index,
        &solver_call,
        &Bytes::default(),
    );
    let (e3, s3) = schnorr_sign_keccak_challenge(&schnorr_sk_hex, msg_solver.as_ref())?;
    let v_call2 = VerificationData {
        id: custody_id,
        state: custody_state,
        timestamp: ts2,
        pubKey: SchnorrCAKey { parity, x },
        sig: SchnorrSignature { e: e3, s: s3 },
        merkleProof: vec![],
    };

    let tx3 = OTCCustody::callConnectorCall {
        connectorType: "OTCIndexConnector".to_string(),
        connectorAddress: deployed_index,
        fixedCallData: solver_call.clone(),
        tailCallData: Bytes::default(),
        v: v_call2,
    };
    let rc3 = otc.callConnector(tx3).send().await?.get_receipt().await?;
    anyhow::ensure!(rc3.status(), "solverUpdate reverted: {:?}", rc3);
    info!("✅ solverUpdate ok: {:?}", rc3.transaction_hash);

    // ---- optional: keep your Deposit watcher running after setup ----
    let _ = wait_for_deposit(&provider, index_addr, poll_ms).await?;

    Ok(())
}

// --------------------------- utilities ---------------------------

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
    Err(anyhow::anyhow!("Timed out waiting for Anvil at {rpc_url}"))
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

/// Wait for the first Deposit and just log it (kept simple here).
async fn wait_for_deposit<P: Provider>(
    provider: &P,
    index_addr: Address,
    poll_ms: u64,
) -> Result<()> {
    let mut last_from = provider.get_block_number().await?;
    let base = Filter::new().address(index_addr);

    loop {
        let tip = provider.get_block_number().await?;
        if tip > last_from {
            let range = base.clone().select(FilterBlockOption::Range {
                from_block: Some(BlockNumberOrTag::Number(last_from + 1)),
                to_block: Some(BlockNumberOrTag::Number(tip)),
            });

            let logs = provider.get_logs(&range).await?;
            for l in logs {
                if let Ok(dl) = l.log_decode::<OTCIndexContract::Deposit>() {
                    let ev = dl.inner;
                    info!(
                        "📥 Deposit: amount={} from={:#x} seq={} aff1={:#x} aff2={:#x}",
                        ev.amount, ev.from, ev.seqNumNewOrderSingle, ev.affiliate1, ev.affiliate2
                    );
                    return Ok(()); // stop after first one for demo
                }
            }
            last_from = tip;
        }
        tokio::time::sleep(std::time::Duration::from_millis(poll_ms)).await;
    }
}
