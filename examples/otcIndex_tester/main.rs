use alloy_rpc_types_eth::TransactionInput;
use anyhow::{anyhow, Context, Result};
use dotenvy::dotenv;
use serde_json::json;
use std::{
    process::{Command, Stdio},
    str::FromStr,
    time::Duration,
};
use tokio::time::sleep;
use tracing::{error, info, warn};
use tracing_subscriber::EnvFilter;
use alloy::network::BlockResponse;
use alloy::primitives::address;
use alloy::{
    primitives::{keccak256, Address, Bytes, B256, U256},
    providers::{ext::AnvilApi, Provider, ProviderBuilder},
    rpc::types::{BlockId, BlockNumberOrTag, Filter, FilterBlockOption, TransactionRequest},
    signers::local::PrivateKeySigner,
    sol,
    sol_types::{SolCall, SolValue},
};
use alloy_consensus::BlockHeader;
use alloy_network::TransactionBuilder;
use alloy_evm_connector::custody_helper::CAHelper;
use alloy_evm_connector::custody_helper::Party;
sol! {
    struct C2ALeafTuple {
        string method;
        uint256 chainId;
        address custody;
        uint8  state;
        bytes  params;
        uint8  pkParity;
        bytes32 pkX;
    }
    struct MyTuple {
        bytes32 leaf;
        uint256 timestamp;
        address destination;
    }
    #[sol(rpc)]
    interface IOTCIndex {
        event Deposit(
            uint256 amount,
            address from,
            uint256 seqNumNewOrderSingle,
            address affiliate1,
            address affiliate2
        );
        event Mint(
            uint256 amount,
            address to,
            uint256 seqNumExecutionReport
        );
        event Withdraw(uint256 amount, address to, bytes executionReport);
        function getCollateralToken() external view returns (address);
        function mint(address target, uint256 amount, uint256 seqNumExecutionReport) external;
    }
}

sol! {
    #[sol(rpc)]
    interface IERC20 {
        function approve(address spender, uint256 value) external returns (bool);
        function balanceOf(address owner) external view returns (uint256);
        function decimals() external view returns (uint8);
        function symbol() external view returns (string);
    }
}

sol! {
    struct SchnorrCAKey { uint8 parity; bytes32 x; }
    struct SchnorrSignature { bytes32 e; bytes32 s; }

    struct VerificationData {
        bytes32 id;
        uint8 state;
        uint256 timestamp;
        SchnorrCAKey pubKey;
        SchnorrSignature sig;
        bytes32[] merkleProof;
    }

    #[sol(rpc)]
    interface IOTCCustody {
        function custodyToAddress(address token, address destination, uint256 amount, VerificationData v) external;
        function addressToCustody(bytes32 id, address token, uint256 amount) external;
        function getCustodyState(bytes32 id) external view returns (uint8);
        function getCustodyBalances(bytes32 id, address token) external view returns (uint256);
        function getCA(bytes32 id) external view returns (bytes32);
    }

    interface IERC20View { function decimals() external view returns (uint8); }

    interface IERC20Transfer {
        function transfer(address to, uint256 amount) external returns (bool);
        function decimals() external view returns (uint8);
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    dotenv().ok();
    let filter = EnvFilter::from_default_env()
        .add_directive("otcIndex_tester=info".parse().unwrap())
        .add_directive("alloy_provider=warn".parse().unwrap());
    tracing_subscriber::fmt().with_env_filter(filter).init();

    // ---- env ----
    let fork_url = std::env::var("BASE_FORK_URL")
        .context("Please set BASE_FORK_URL to a Base RPC (archive preferred)")?;
    let index_addr: Address = env_addr("OTC_INDEX_ADDR")?;
    let custody_addr: Address = env_addr("OTC_CUSTODY_ADDR")?;
    let operator_pk: PrivateKeySigner = env_pk("OPERATOR_PK")?;

    let anvil_http =
        std::env::var("ANVIL_HTTP").unwrap_or_else(|_| "http://127.0.0.1:8545".to_string());
    let poll_ms: u64 = std::env::var("POLL_MS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(1000);
    let frontend: Address = std::env::var("FRONTEND_ADDR")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(Address::ZERO);
    let exec_price =
        parse_u256_decimal(&std::env::var("EXEC_PRICE").unwrap_or_else(|_| "1000000".to_string()))?;
    let exec_time_override =
        parse_u256_decimal(&std::env::var("EXEC_TIME").unwrap_or_else(|_| "0".to_string()))?;

    // ---- 1) discover deploy blocks on live Base ----
    let live = ProviderBuilder::new().connect(&fork_url).await?;
    let latest = live.get_block_number().await?;
    let idx_block = find_deploy_block(&live, index_addr, latest).await?;
    let cty_block = find_deploy_block(&live, custody_addr, latest).await?;
    let fork_block = idx_block.max(cty_block);
    info!("Using fork from Base block #{fork_block} (OTCIndex at #{idx_block}, OTCCustody at #{cty_block}).");

    // ---- 2) start Anvil fork ----
    info!("Spawning anvil fork at block #{fork_block} …");
    let mut anvil = Command::new("anvil")
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

    // wait for JSON-RPC to come up
    wait_for_rpc(&anvil_http).await?;
    info!("Anvil fork online at {}", anvil_http);

    let signer: PrivateKeySigner = std::env::var("OPERATOR_PK")?.parse()?;
    let provider = ProviderBuilder::new()
        // .wallet(signer)
        .connect("http://127.0.0.1:8545")
        .await?;

    provider.anvil_auto_impersonate_account(true).await?;
    // contracts
    let index = IOTCIndex::new(index_addr, provider.clone());
    let custody = IOTCCustody::new(custody_addr, provider.clone());
    let collateral_fb = index.getCollateralToken().call().await?.0;
    let collateral: Address = collateral_fb.into();

    let usdc: Address = "0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913".parse()?; // native USDC on Base
    let rpc_url = "http://127.0.0.1:8545"; // or whatever you're using
    let bare = ProviderBuilder::new().connect(rpc_url).await?;
    // ** Whale address from env
    let whale: Address = std::env::var("WHALE")
        .expect("Set WHALE env to a Base USDC whale")
        .parse()
        .expect("Invalid WHALE");

    bare.anvil_impersonate_account(whale).await?;
    let whale_eth = U256::from(10_000u64) * exp10(18);
    bare.anvil_set_balance(whale, whale_eth).await?;

    let usdc_contract = IERC20::new(usdc, bare.clone());
    let decimals: u8 = usdc_contract.decimals().call().await?;
    fn exp10(d: u8) -> U256 {
        let mut x = U256::from(1u8);
        for _ in 0..d {
            x = x * U256::from(10u8);
        }
        x
    }
    let amount_units = U256::from(10_000u64) * exp10(decimals);

    // ** Default 10 Anvil accounts
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

    // ** Impersonate whale on the fork
    bare.anvil_impersonate_account(whale).await?;

    // ** Transfer 10,000 collateral to each recipient
    for &to in &recipients {
        let calldata: Bytes = IERC20Transfer::transferCall {
            to,
            amount: amount_units,
        }
        .abi_encode()
        .into();

        let tx = TransactionRequest::default()
            .with_from(whale)
            .with_to(usdc)
            .with_input(calldata)
            .with_gas_limit(120_000u64);

        let pending = bare.send_transaction(tx).await?;
        let receipt = pending.get_receipt().await?;
        if !receipt.status() {
            error!("USDC transfer to {to:#x} failed: {:?}", receipt);
            anyhow::bail!("USDC transfer failed");
        }
        info!("10_000 USDC -> {to:#x} | tx {:?}", receipt.transaction_hash);
    }

    // ** Stop impersonating (optional)
    bare.anvil_stop_impersonating_account(whale).await?;

    // ---- 3) wait for real Deposit ----
    let start_block = provider.get_block_number().await?;
    let filter = Filter::new()
        .address(index_addr)
        .event("Deposit(uint256,address,bytes,address)")
        .select(FilterBlockOption::Range {
            from_block: Some(BlockNumberOrTag::Number(start_block)),
            to_block: None,
        });

    info!(
        "Waiting for Deposit on OTCIndex {} from block #{} …",
        index_addr, start_block
    );
    let (amount, from, quote, affiliate, seen_at) =
        wait_for_deposit(&provider, &filter, index_addr, poll_ms, collateral).await?;

    Ok(())
}

async fn wait_for_rpc(rpc_url: &str) -> Result<()> {
    let client = reqwest::Client::new();
    for _ in 0..60 {
        let res = client
            .post(rpc_url)
            .json(&json!({"jsonrpc":"2.0","id":1,"method":"eth_chainId","params":[]}))
            .send()
            .await;
        if res.is_ok() {
            return Ok(());
        }
        sleep(Duration::from_millis(500)).await;
    }
    Err(anyhow::anyhow!("Timed out waiting for Anvil at {rpc_url}"))
}

fn abi_encode_address(addr: Address) -> Bytes {
    let mut out = vec![0u8; 32];
    out[12..].copy_from_slice(addr.as_slice());
    out.into()
}

fn parse_b256(s: &str) -> Result<B256> {
    let t = s.trim();
    let t = t.strip_prefix("0x").unwrap_or(t);
    if t.len() != 64 {
        return Err(anyhow!("expected 32-byte hex (64 chars), got {}", t.len()));
    }
    if !t.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(anyhow!("non-hex character in OTC_CUSTODY_ID"));
    }
    // B256 implements FromStr on "0x…"
    Ok(format!("0x{t}").parse::<B256>()?)
}

fn build_c2a_leaf(
    custody_addr: Address,
    chain_id: u64,
    state_u8: u8,
    destination: Address,
    pk_parity: u8,
    pk_x: B256,
) -> B256 {
    // params = abi.encode(destination)
    let encoded_params: Bytes = abi_encode_address(destination);

    // inner = keccak256(abi.encode(
    //   "custodyToAddress", chainId, custody, state, params, pkParity, pkX
    // ))
    let value = C2ALeafTuple {
        method: "custodyToAddress".to_string(),
        chainId: U256::from(chain_id),
        custody: custody_addr,
        state: state_u8,
        params: encoded_params,
        pkParity: pk_parity,
        pkX: pk_x,
    };
    let inner = keccak256(value.abi_encode());

    keccak256(inner)
}

pub async fn make_merkle_passing_vdata<P: Provider>(
    p: &P,
    custody_addr: Address,
    chain_id: u64,
    destination: Address,
) -> Result<VerificationData> {
    // 1) fresh timestamp from latest block
    let now: U256 = p
        .get_block_by_number(BlockNumberOrTag::Latest)
        .await?
        .map(|b| U256::from(b.header().timestamp()))
        .unwrap_or(U256::ZERO);

    // 2) Build a CA tree with ONE item: custodyToAddress(destination)
    //    Make sure the pubKey you put in the leaf == pubKey you return in VerificationData.
    let party = Party { parity: 0, x: B256::ZERO };
    let state: u8 = 0;

    let mut ca = CAHelper::new(chain_id as u32, custody_addr);
    let leaf_index = ca.custody_to_address(destination, state, party.clone());

    // 3) Merkle root -> VerificationData.id
    let id_bytes32 = ca.get_ca_root();              // [u8;32]
    let id = B256::from_slice(&id_bytes32);         // bytes32 root

    // 4) Leaf (double-keccak of abi-encoded item) -> used in e computation
    let leaf_bytes = CAHelper::compute_leaf(&ca.get_ca_items()[leaf_index]);
    let leaf = B256::from_slice(leaf_bytes.as_ref());

    // 5) Merkle proof for this leaf
    let proof_arrs = ca.get_merkle_proof(leaf_index);          // Vec<[u8;32]>
    let merkle_proof: Vec<B256> = proof_arrs
        .iter()
        .map(|a| B256::from_slice(a))
        .collect();

    // 6) Demo signature (leave as-is if you’re purposely bypassing Schnorr)
    let e = B256::from(keccak256(
        MyTuple { leaf, timestamp: now, destination }.abi_encode()
    ));
    let s = B256::from([0x33; 32]);

    Ok(VerificationData {
        id,
        state,
        timestamp: now,
        pubKey: SchnorrCAKey {
            parity: party.parity,
            x: party.x,
        },
        sig: SchnorrSignature { e, s },
        merkleProof: merkle_proof,
    })
}

async fn try_custody_to_address<P: Provider>(
    p: &P,
    custody_addr: Address,
    token: Address,
    to: Address,
    amount: U256,
) -> Result<()> {
    let now: U256 = p
        .get_block_by_number(BlockNumberOrTag::Latest)
        .await?
        .map(|b| U256::from(b.header().timestamp()))
        .unwrap_or(U256::ZERO);

    
    let chain_id = 8453u64;
    let v = make_merkle_passing_vdata(&p, custody_addr, chain_id, to).await?;
    let _amount: U256 = U256::from(0);
    let calldata: Bytes = IOTCCustody::custodyToAddressCall {
        token,
        destination: to,
        amount: _amount,
        v,
    }
    .abi_encode()
    .into();

    p.anvil_impersonate_account(custody_addr).await?;
    let ten_k_eth = U256::from(10_000u64) * U256::from(10).pow(U256::from(18));
    let _ = p.anvil_set_balance(custody_addr, ten_k_eth).await;

    match p
        .call(
            TransactionRequest::default()
                .to(custody_addr)
                .input(calldata.clone().into()),
        )
        .await
    {
        Ok(_) => warn!("custodyToAddress eth_call succeeded"),
        Err(e) => info!("❌ custodyToAddress eth_call reverted (expected in demo): {e:?}"),
    }
    p.anvil_stop_impersonating_account(custody_addr).await?;
    Ok(())
}

async fn mint_from_custody<P: Provider>(
    p: &P,
    custody_addr: Address,
    index_addr: Address,
    to: Address,
    amount: U256,
    seq: U256,
) -> anyhow::Result<()> {
    let index_price: U256 = std::env::var("INDEX_PRICE")
        .expect("INDEX_PRICE environment variable not set")
        .parse()
        .expect("INDEX_PRICE must be a valid number");

    fn pow10(d: u32) -> U256 {
        let mut x = U256::from(1);
        for _ in 0..d {
            x = x * U256::from(10);
        }
        x
    }
    let mint_amount = amount * pow10(18) / index_price;

    p.anvil_impersonate_account(custody_addr).await?;
    let ten_k_eth = U256::from(10_000u64) * U256::from(10).pow(U256::from(18));
    let _ = p.anvil_set_balance(custody_addr, ten_k_eth).await;

    let call: Bytes = IOTCIndex::mintCall {
        target: to,
        amount: mint_amount,
        seqNumExecutionReport: seq,
    }
    .abi_encode()
    .into();

    let tx = TransactionRequest::default()
        .from(custody_addr)
        .to(index_addr)
        .input(call.into())
        .gas_limit(300_000u64);

    let pending = p.send_transaction(tx).await?;
    let rc = pending.get_receipt().await?;
    if !rc.status() {
        anyhow::bail!("mint() reverted: {:?}", rc);
    }
    let mint_as_f64 = mint_amount.to_string().parse::<f64>().unwrap() / 1e18;
    // Enhanced logging with all relevant information
    tracing::info!(
        "✅ Mint successful - \
        Tx: {:?}, \
        Deposit amount: {:?}, \
        Index price: {:?}, \
        Mint amount: {:?}, \
        To: {:?}",
        rc.transaction_hash,
        amount,
        index_price,
        mint_as_f64,
        to
    );

    p.anvil_stop_impersonating_account(custody_addr).await?;
    Ok(())
}

/// Waits for the very first Deposit and returns its fields + the block number it was seen at.
async fn wait_for_deposit<P: Provider>(
    provider: &P,
    base_filter: &Filter,
    index_addr: Address,
    poll_ms: u64,
    collateral: Address,
) -> Result<(U256, Address, Bytes, Address, u64)> {
    let mut last_from = current_block(provider).await?;
    let base = Filter::new().address(index_addr);
    loop {
        let tip = current_block(provider).await?;
        if tip > last_from {
            let range = base.clone().select(FilterBlockOption::Range {
                from_block: Some(BlockNumberOrTag::Number(last_from + 1)),
                to_block: Some(BlockNumberOrTag::Number(tip)),
            });

            let logs = provider.get_logs(&range).await?;
            for l in logs {
                if let Ok(dl) = l.log_decode::<IOTCIndex::Deposit>() {
                    let ev = dl.inner;
                    info!(
                        "📥 Deposit: amount={} from={:#x} seq={} aff1={:#x} aff2={:#x}",
                        ev.amount, ev.from, ev.seqNumNewOrderSingle, ev.affiliate1, ev.affiliate2
                    );
                    let custody_addr: Address = std::env::var("OTC_CUSTODY_ADDR")
                        .expect("OTC_CUSTODY_ADDR")
                        .parse()?;
                    if let Err(e) = try_custody_to_address(
                        &provider,
                        custody_addr,
                        collateral,
                        ev.from,
                        ev.amount,
                    )
                    .await
                    {
                        warn!("custodyToAddress preview failed: {e:?}");
                    }

                    // 2) mint to depositor (demo path)
                    if let Err(e) = mint_from_custody(
                        &provider,
                        custody_addr,
                        index_addr,
                        ev.from,
                        ev.amount,
                        ev.seqNumNewOrderSingle.into(),
                    )
                    .await
                    {
                        warn!("mint_from_custody failed: {e:?}");
                    }
                    continue;
                }
            }
            last_from = tip;
        }
        tokio::time::sleep(std::time::Duration::from_millis(poll_ms)).await;
    }
}

async fn current_block<P: Provider>(provider: &P) -> Result<u64> {
    Ok(provider.get_block_number().await?)
}

async fn find_deploy_block<P: Provider>(p: &P, addr: Address, latest: u64) -> Result<u64> {
    let mut lo = 1u64;
    let mut hi = latest;
    let mut ans = latest;
    while lo <= hi {
        let mid = (lo + hi) / 2;
        let code = p
            .get_code_at(addr)
            .block_id(BlockId::Number(BlockNumberOrTag::Number(mid))) // mid: u64
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
    let s = std::env::var(key).with_context(|| format!("missing env {key}"))?;
    Address::from_str(s.trim()).with_context(|| format!("invalid {key}"))
}

fn env_pk(key: &str) -> Result<PrivateKeySigner> {
    let s = std::env::var(key).with_context(|| format!("missing env {key}"))?;
    let pk: PrivateKeySigner = s.trim().parse().context("invalid hex pk")?;
    Ok(pk)
}

fn parse_u256_decimal(s: &str) -> anyhow::Result<U256> {
    let s = s.trim();
    if s.is_empty() {
        return Ok(U256::ZERO);
    }
    if s.starts_with('-') {
        return Err(anyhow::anyhow!("negative not allowed"));
    }
    let mut acc = U256::ZERO;
    for ch in s.bytes() {
        if !(b'0'..=b'9').contains(&ch) {
            return Err(anyhow::anyhow!("invalid decimal: {s}"));
        }
        let d = (ch - b'0') as u8;
        acc = acc * U256::from(10u8) + U256::from(d);
    }
    Ok(acc)
}

fn keccak_pair(left: B256, right: B256) -> B256 {
    let mut buf = [0u8; 64];
    buf[..32].copy_from_slice(left.as_slice());
    buf[32..].copy_from_slice(right.as_slice());
    keccak256(&buf)
}

pub fn merkle_proof_positional(leaves: &[B256], mut index: usize) -> anyhow::Result<Vec<B256>> {
    if leaves.is_empty() {
        anyhow::bail!("no leaves");
    }
    if index >= leaves.len() {
        anyhow::bail!("index {} out of bounds (0..{})", index, leaves.len().saturating_sub(1));
    }
    if leaves.len() == 1 {
        return Ok(vec![]); 
    }

    let mut proof = Vec::<B256>::new();
    let mut layer: Vec<B256> = leaves.to_vec();

    while layer.len() > 1 {
        let is_right = index % 2 == 1;
        let sibling_idx = if is_right { index - 1 } else { index + 1 };

        // push sibling if it exists in this layer
        if sibling_idx < layer.len() {
            proof.push(layer[sibling_idx]);
        }

        // build next layer
        let mut next = Vec::<B256>::with_capacity((layer.len() + 1) / 2);
        for i in (0..layer.len()).step_by(2) {
            let parent = if i + 1 < layer.len() {
                keccak_pair(layer[i], layer[i + 1]) 
            } else {
                layer[i]
            };
            next.push(parent);
        }
        layer = next;
        index /= 2;
    }

    Ok(proof)
}

pub fn compute_root_from_indexed_proof(mut leaf: B256, proof: &[B256], mut index: usize) -> B256 {
    for sib in proof {
        leaf = if index % 2 == 0 {
            keccak_pair(leaf, *sib) 
        } else {
            keccak_pair(*sib, leaf) 
        };
        index >>= 1;
    }
    leaf
}