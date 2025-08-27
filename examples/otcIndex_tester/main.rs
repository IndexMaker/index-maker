use alloy::{
    primitives::{address, keccak256, Address, Bytes, FixedBytes, B256, U256},
    providers::{ext::AnvilApi, Provider, ProviderBuilder},
    rpc::types::{BlockId, BlockNumberOrTag, Filter, FilterBlockOption},
    sol_types::SolValue,
};
use alloy_chain_connector::util::amount_converter::AmountConverter;
use alloy_consensus::BlockHeader;
use alloy_network::BlockResponse;
use anyhow::{anyhow, Context, Result};
use ca_helper::contracts::{
    IOTCIndex::Deposit, IndexFactory, OTCCustody, SchnorrCAKey, SchnorrSignature, VerificationData,
    ERC20,
};
use ca_helper::custody_helper::{CAHelper, Party};
use dotenvy::dotenv;
use k256::elliptic_curve::sec1::ToEncodedPoint;
use k256::elliptic_curve::{bigint::U256 as ECUint, ops::Reduce};
use k256::ProjectivePoint;
use k256::{elliptic_curve::PrimeField, FieldBytes, Scalar};
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

// ---------- helpers: key → (parity, x) for Schnorr pubkey ----------
fn pubkey_parity_27_28_x(sk_hex: &str) -> anyhow::Result<(u8, B256, [u8; 32])> {
    use k256::ecdsa::SigningKey;

    let sk_vec = hex::decode(sk_hex.trim_start_matches("0x"))?;
    let sk32: [u8; 32] = sk_vec
        .as_slice()
        .try_into()
        .map_err(|_| anyhow!("bad sk length"))?;

    let sk = SigningKey::from_slice(&sk32)?;
    let vk = sk.verifying_key();

    let ep = vk.to_encoded_point(true); // 33 bytes: 0x02/0x03 || X
    let tag = ep.as_bytes()[0];
    let parity = if tag == 0x02 { 27u8 } else { 28u8 }; // this is required from our Solidity contracts
    let x_bytes = &ep.as_bytes()[1..33];
    Ok((parity, B256::from_slice(x_bytes), sk32))
}

fn schnorr_sign_per_contract(
    sk32: [u8; 32],
    parity: u8, // 27/28
    px: B256,   // pubkey X
    message_data: &[u8],
) -> anyhow::Result<(B256, B256)> {
    let x = Scalar::from_repr(FieldBytes::from(sk32))
        .into_option()
        .ok_or_else(|| anyhow::anyhow!("sk out of range"))?;

    // message = keccak256(messageData)
    let message = B256::from(keccak256(message_data));

    // deterministic k = reduce(keccak256(sk || message))
    let mut kin = Vec::with_capacity(64);
    kin.extend_from_slice(&sk32);
    kin.extend_from_slice(message.as_slice());
    let k = Scalar::reduce(ECUint::from_be_slice(keccak256(&kin).as_slice()));

    // R = k·G, ethereum address of R (keccak of uncompressed XY, take last 20 bytes)
    let r_aff = (ProjectivePoint::GENERATOR * k).to_affine();
    let r_uncompressed = r_aff.to_encoded_point(false); // 0x04 || X || Y (65 bytes)
    let r_hash = keccak256(&r_uncompressed.as_bytes()[1..]); // hash 64-byte X||Y
    let r_addr = Address::from_slice(&r_hash[12..]); // last 20 bytes

    // e = keccak256(abi.encodePacked(R_address, parity, px, message))
    let parity_b1: FixedBytes<1> = [parity].into();
    let e_bytes: Bytes = SolValue::abi_encode_packed(&(r_addr, parity_b1, px, message)).into();
    let e_b256 = B256::from(keccak256(e_bytes.as_ref()));
    let e = Scalar::reduce(ECUint::from_be_slice(e_b256.as_slice()));

    // s = k + e*x  (mod Q)
    let s = k + e * x;

    Ok((e_b256, B256::from_slice(s.to_bytes().as_slice())))
}

fn deploy_connector_message(
    ts: U256,
    id: B256,
    connector_type: &str,
    factory: Address,
    data: &Bytes,
) -> Bytes {
    let data_hash = B256::from(keccak256(data));
    SolValue::abi_encode_packed(&(
        ts,
        "deployConnector".to_string(),
        id,
        connector_type.to_string(),
        factory,
        data_hash,
    ))
    .into()
}

fn call_connector_message(
    ts: U256,
    id: B256,
    connector_type: &str,
    connector_addr: Address,
    fixed: &Bytes,
    tail: &Bytes,
) -> Bytes {
    let fixed_hash = B256::from(keccak256(fixed));
    let tail_hash = B256::from(keccak256(tail));
    SolValue::abi_encode_packed(&(
        ts,
        "callConnector".to_string(),
        id,
        connector_type.to_string(),
        connector_addr,
        fixed_hash,
        tail_hash,
    ))
    .into()
}

fn custody_to_address_message(
    ts: U256,
    id: B256,
    token: Address,
    destination: Address,
    amount: U256,
) -> Bytes {
    SolValue::abi_encode_packed(&(
        ts,
        "custodyToAddress".to_string(),
        id,
        token,
        destination,
        amount,
    ))
    .into()
}

fn encode_mint_call(to: Address, amount: U256, seq: U256) -> Bytes {
    let sel = keccak256(b"mint(address,uint256,uint256)");
    let mut out = Vec::with_capacity(4 + 32 * 3);
    out.extend_from_slice(&sel[..4]);
    let enc = SolValue::abi_encode_params(&(to, amount, seq));
    out.extend_from_slice(&enc);
    out.into()
}

#[tokio::main]
async fn main() -> Result<()> {
    dotenv().ok();
    let filter = EnvFilter::from_default_env()
        .add_directive("otcIndex_tester=info".parse().unwrap())
        .add_directive("alloy_provider=warn".parse().unwrap());
    tracing_subscriber::fmt().with_env_filter(filter).init();

    let fork_url = env::var("BASE_FORK_URL").context("Please set BASE_FORK_URL")?;
    let custody_addr: Address = env_addr("OTC_CUSTODY_ADDR")?;
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
        let cty_block = find_deploy_block(&live, custody_addr, latest).await?;
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

    // === Realistic ACL + Schnorr + deployConnector + callConnector     ===

    let index_factory_addr: Address = env_addr("INDEX_FACTORY_ADDR")?;
    let schnorr_sk_hex = env::var("SCHNORR_SK").unwrap_or_else(|_| {
        "0x1111111111111111111111111111111111111111111111111111111111111111".into()
    });
    let chain_id: u64 = 8453;

    let operator: Address = env_addr("OPERATOR_ADDR")?;
    // 1) Build CA/ACL for deployConnector ("OTCIndex") and compute CustodyID
    let (parity, px, sk32) = pubkey_parity_27_28_x(&schnorr_sk_hex)?;
    let party = Party { parity, x: px };
    let custody_state: u8 = 0;

    // index params (mirror AB's ts script)
    let index_params = (
        "Factory Index".to_string(),
        "FI".to_string(),
        usdc,
        U256::from(6u8),
        U256::from(100u64),
        U256::from(200u64),
        u256("1000000000000000000000")?, // 1000 * 1e18
        u256("1000000000000000000000")?,
        U256::from(51u8),
        U256::from(86400u64),
        u256("1000000000000000000000")?, // 100 * 1e18
    );
    let deploy_data: Bytes = SolValue::abi_encode_params(&index_params).into();

    let mut ca = CAHelper::new(chain_id as u32, custody_addr);
    // (A) deployConnector action leaf
    let leaf_idx_deploy = ca.deploy_connector(
        "OTCIndex",
        index_factory_addr,
        deploy_data.as_ref(),
        custody_state,
        party.clone(),
    );

    // (B) custodyToAddress action leaf — send to your operator (or any address you want)
    let receiver: Address = operator;
    let leaf_idx_c2a = ca.custody_to_address(receiver, custody_state, party.clone());

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
    let (e, s) = schnorr_sign_per_contract(sk32, parity, px, msg.as_ref())?;

    let proof_deploy: Vec<B256> = ca
        .get_merkle_proof(leaf_idx_deploy)
        .into_iter()
        .map(|arr| B256::from_slice(&arr))
        .collect();

    let proof_c2a: Vec<B256> = ca
        .get_merkle_proof(leaf_idx_c2a)
        .into_iter()
        .map(|arr| B256::from_slice(&arr))
        .collect();

    let v_deploy = VerificationData {
        id: custody_id,
        state: custody_state,
        timestamp: latest_ts,
        pubKey: SchnorrCAKey { parity, x: px },
        sig: SchnorrSignature { e, s },
        merkleProof: proof_deploy.clone(),
    };

    // 3) Call deployConnector on OTCCustody
    let otc = OTCCustody::new(custody_addr, provider.clone());

    // gas for custodian on anvil
    provider.anvil_impersonate_account(custody_addr).await?;
    let _ = provider
        .anvil_set_balance(
            custody_addr,
            u256("10000000000000000000000")?, // 10k ETH
        )
        .await;
    provider
        .anvil_stop_impersonating_account(custody_addr)
        .await?;

    let rc = otc
        .deployConnector(
            "OTCIndex".to_string(),
            index_factory_addr,
            deploy_data.clone(),
            v_deploy,
        )
        .from(operator)
        .send()
        .await?
        .get_receipt()
        .await?;
    anyhow::ensure!(rc.status(), "deployConnector reverted: {:?}", rc);
    info!("✅ deployConnector ok: {:?}", rc.transaction_hash);

    // 4) Read IndexDeployed to get deployed index
    let mut deployed_index = Address::ZERO;

    for lg in rc.logs() {
        let log_addr = hex::encode(lg.address().as_slice()); // lowercase
        let factory_addr = hex::encode(index_factory_addr.as_slice()); // lowercase
        if log_addr == factory_addr {
            if let Ok(parsed) = lg.log_decode::<IndexFactory::IndexDeployed>() {
                deployed_index = parsed.inner.indexAddress;
                break;
            }
        }
    }

    anyhow::ensure!(
        deployed_index != Address::ZERO,
        "IndexDeployed not found in receipt — check factory address/ABI (logs dumped above)"
    );

    info!("🆕 OTCIndex deployed at: {deployed_index:#x}");

    // auto-whitelist check
    let whitelisted = otc.isConnectorWhitelisted(deployed_index).call().await?;
    info!("Index auto-whitelisted: {}", whitelisted);

    // ===== custodyToAddress (deposit then withdraw-to-address via Schnorr) =====
    let c2a_amount_units = usdc_converter
        .from_amount(dec!(100))
        .map_err(|e| anyhow!(e))?;

    let usdc_ext = ERC20::new(usdc, &provider);
    // Approve USDC to Custody so it can transferFrom(operator)
    let rc_app = usdc_ext
        .approve(custody_addr, c2a_amount_units)
        .from(operator)
        .send()
        .await?
        .get_receipt()
        .await?;
    anyhow::ensure!(rc_app.status(), "USDC approve failed: {:?}", rc_app);

    // Deposit into custody under this custodyId
    let rc_dep = otc
        .addressToCustody(custody_id, usdc, c2a_amount_units)
        .from(operator)
        .send()
        .await?
        .get_receipt()
        .await?;
    anyhow::ensure!(rc_dep.status(), "addressToCustody failed: {:?}", rc_dep);

    // Build & sign custodyToAddress message with fresh timestamp
    let ts_c2a: U256 = chain_now_ts(&provider).await?;

    let msg_c2a = custody_to_address_message(ts_c2a, custody_id, usdc, receiver, c2a_amount_units);

    let (e_c2a, s_c2a) = schnorr_sign_per_contract(sk32, parity, px, msg_c2a.as_ref())?;

    let v_c2a = VerificationData {
        id: custody_id,
        state: custody_state,
        timestamp: ts_c2a,
        pubKey: SchnorrCAKey { parity, x: px },
        sig: SchnorrSignature { e: e_c2a, s: s_c2a },
        merkleProof: proof_c2a.clone(), // for the custodyToAddress leaf
    };

    // Execute custodyToAddress (moves funds from custody to `receiver`)
    let rc_c2a = otc
        .custodyToAddress(usdc, receiver, c2a_amount_units, v_c2a)
        .from(operator)
        .send()
        .await?
        .get_receipt()
        .await?;
    anyhow::ensure!(rc_c2a.status(), "custodyToAddress reverted: {:?}", rc_c2a);

    info!("✅ custodyToAddress ok: {:?}", rc_c2a.transaction_hash);

    let bal_after = usdc_contract.balanceOf(receiver).call().await?;
    info!(
        "USDC balance(receiver) after custodyToAddress: {}",
        bal_after
    );

    // 5) callConnector: curatorUpdate + solverUpdate (empty proof when whitelisted)
    let ts1 = chain_now_ts(&provider).await?;
    let weights1 = Bytes::from(vec![0u8; 32]);
    let price1 = u256("120000000000000000000")?;
    let curator_call = encode_with_selector(
        "curatorUpdate(uint256,bytes,uint256)",
        ts1,
        &weights1,
        price1,
    );

    let msg_curator = call_connector_message(
        ts1,
        custody_id,
        "OTCIndexConnector",
        deployed_index,
        &curator_call,
        &Bytes::default(),
    );
    let (e2, s2) = schnorr_sign_per_contract(sk32, parity, px, msg_curator.as_ref())?;

    let v_call1 = VerificationData {
        id: custody_id,
        state: custody_state,
        timestamp: ts1,
        pubKey: SchnorrCAKey { parity, x: px },
        sig: SchnorrSignature { e: e2, s: s2 },
        merkleProof: vec![], // index is already in whitelisted so empty proof OK
    };

    let rc2 = otc
        .callConnector(
            "OTCIndexConnector".to_string(),
            deployed_index,
            curator_call.clone(),
            Bytes::default(),
            v_call1,
        )
        .from(operator)
        .send()
        .await?
        .get_receipt()
        .await?;

    anyhow::ensure!(rc2.status(), "curatorUpdate reverted: {:?}", rc2);
    info!("✅ curatorUpdate ok: {:?}", rc2.transaction_hash);

    let ts2 = chain_now_ts(&provider).await?;
    let weights2 = Bytes::from(vec![1u8; 32]);
    let price2 = u256("130000000000000000000")?;
    let solver_call = encode_with_selector(
        "solverUpdate(uint256,bytes,uint256)",
        ts2,
        &weights2,
        price2,
    );

    let msg_solver = call_connector_message(
        ts2,
        custody_id,
        "OTCIndexConnector",
        deployed_index,
        &solver_call,
        &Bytes::default(),
    );
    let (e3, s3) = schnorr_sign_per_contract(sk32, parity, px, msg_solver.as_ref())?;
    let v_call2 = VerificationData {
        id: custody_id,
        state: custody_state,
        timestamp: ts2,
        pubKey: SchnorrCAKey { parity, x: px },
        sig: SchnorrSignature { e: e3, s: s3 },
        merkleProof: vec![],
    };

    let rc3 = otc
        .callConnector(
            "OTCIndexConnector".to_string(),
            deployed_index,
            solver_call.clone(),
            Bytes::default(),
            v_call2,
        )
        .from(operator)
        .send()
        .await?
        .get_receipt()
        .await?;

    anyhow::ensure!(rc3.status(), "solverUpdate reverted: {:?}", rc3);
    info!("✅ solverUpdate ok: {:?}", rc3.transaction_hash);

    info!("Waiting deposit event from Client side...");
    let _ = wait_for_deposit_and_mint(
        &provider,
        deployed_index,
        custody_addr,
        poll_ms,
        custody_id,
        custody_state,
        parity,
        px,
        sk32,
        6,
    )
    .await?;

    Ok(())
}

// --------------------------- my helpers ---------------------------
fn encode_with_selector(sig: &str, ts: U256, weights: &Bytes, price: U256) -> Bytes {
    let sel = keccak256(sig.as_bytes());
    let mut out = Vec::with_capacity(4 + 128);
    out.extend_from_slice(&sel[..4]);

    let enc = SolValue::abi_encode_params(&(ts, weights.clone(), price));
    out.extend_from_slice(&enc);
    out.into()
}

async fn chain_now_ts<P: Provider>(p: &P) -> anyhow::Result<U256> {
    let now = p
        .get_block_by_number(BlockNumberOrTag::Latest)
        .await?
        .map(|b| U256::from(b.header().timestamp()))
        .unwrap_or(U256::ZERO);
    Ok(now)
}

fn u256(s: &str) -> anyhow::Result<U256> {
    U256::from_str(s).map_err(|e| anyhow!(e))
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

fn pow10(d: u32) -> U256 {
    let mut x = U256::from(1u8);
    for _ in 0..d {
        x = x * U256::from(10u8);
    }
    x
}

fn read_index_price() -> anyhow::Result<U256> {
    let s = std::env::var("INDEX_PRICE").context("INDEX_PRICE env var not set")?;
    U256::from_str(&s).map_err(|e| anyhow!(e))
}

async fn pending_nonce<P: Provider>(p: &P, from: Address) -> anyhow::Result<u64> {
    let n: u64 = p
        .get_transaction_count(from)
        .block_id(BlockId::Number(BlockNumberOrTag::Pending))
        .await?;
    Ok(n)
}

/// Wait for the Deposit events and Mint immeidately for capturing on Client.
async fn wait_for_deposit_and_mint<P: Provider>(
    provider: &P,
    index_addr: Address,
    custody_addr: Address,
    poll_ms: u64,
    custody_id: B256,
    custody_state: u8,
    parity: u8,
    px: B256,
    sk32: [u8; 32],
    collateral_decimals: u8,
) -> Result<()> {
    let index_price = read_index_price()?;
    let one_e18 = pow10(18);
    let mut last_from = provider.get_block_number().await?;
    let base = Filter::new().address(index_addr);
    let otc = OTCCustody::new(custody_addr, provider.clone());
    loop {
        let tip = provider.get_block_number().await?;
        if tip > last_from {
            let range = base.clone().select(FilterBlockOption::Range {
                from_block: Some(BlockNumberOrTag::Number(last_from + 1)),
                to_block: Some(BlockNumberOrTag::Number(tip)),
            });

            let logs = provider.get_logs(&range).await?;
            for l in logs {
                if let Ok(dl) = l.log_decode::<Deposit>() {
                    let ev = dl.inner;
                    info!(
                        "📥 Deposit: amount={} from={:#x} seq={} aff1={:#x} aff2={:#x}",
                        ev.amount, ev.from, ev.seqNumNewOrderSingle, ev.affiliate1, ev.affiliate2
                    );

                    // Mint after deposit
                    let amount18 = if collateral_decimals < 18 {
                        ev.amount * pow10((18 - collateral_decimals as u32) as u32)
                    } else if collateral_decimals > 18 {
                        ev.amount / pow10((collateral_decimals as u32 - 18) as u32)
                    } else {
                        ev.amount
                    };

                    let mint_amount = amount18.saturating_mul(one_e18) / index_price;

                    let mint_call = encode_mint_call(ev.from, mint_amount, ev.seqNumNewOrderSingle);
                    //  get cusody owner for Mint allow
                    let custody_owner: Address = otc.getCustodyOwner(custody_id).call().await?;
                    let ts: U256 = chain_now_ts(&provider).await?;
                    let nonce = pending_nonce(&provider, custody_owner).await?;
                    let msg = call_connector_message(
                        ts,
                        custody_id,
                        "OTCIndexConnector",
                        index_addr,
                        &mint_call,
                        &Bytes::default(),
                    );

                    // Sign per your contract’s Schnorr flow (27/28)
                    let (e, s) = schnorr_sign_per_contract(sk32, parity, px, msg.as_ref())?;

                    let v = VerificationData {
                        id: custody_id,
                        state: custody_state,
                        timestamp: ts,
                        pubKey: SchnorrCAKey { parity, x: px },
                        sig: SchnorrSignature { e, s },
                        merkleProof: vec![], // OK if connector is whitelisted
                    };

                    //   Send via custody → connector (use SAME client; set from)
                    let rc = otc
                        .callConnector(
                            "OTCIndexConnector".to_string(),
                            index_addr,
                            mint_call,
                            Bytes::default(),
                            v,
                        )
                        .from(custody_owner)
                        .nonce(nonce)
                        .send()
                        .await?
                        .get_receipt()
                        .await?;

                    anyhow::ensure!(rc.status(), "mint via callConnector reverted: {:?}", rc);

                    info!(
                        "🎟️ Minted {} to {:#x} (seq {}) | tx {:?}",
                        mint_amount, ev.from, ev.seqNumNewOrderSingle, rc.transaction_hash
                    );

                    continue;
                }
            }
            last_from = tip;
        }
        tokio::time::sleep(std::time::Duration::from_millis(poll_ms)).await;
    }
}
