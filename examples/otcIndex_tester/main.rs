//! RUST_LOG=info cargo run --example otc_alloy_isolated
//!
//! Required env:
//!   BASE_FORK_URL   = "https://mainnet.base.org"     # or your archival Base RPC
//!   OTC_INDEX_ADDR  = "0x..."                        # deployed OTCIndex (live on Base)
//!   OTC_CUSTODY_ADDR= "0x..."                        # deployed OTCCustody (live on Base)
//!   OPERATOR_PK     = "0x..."                        # hex private key (for Alloy signing demo)
//!
//! Optional env:
//!   ANVIL_HTTP      = "http://127.0.0.1:8545"        # where fork will be exposed
//!   POLL_MS         = "1000"                         # polling interval ms
//!   FRONTEND_ADDR   = "0x..."                        # address to attribute as 'frontend' in Mint
//!   EXEC_PRICE      = "1000000"                      # arbitrary execution price to log in Mint
//!   EXEC_TIME       = "0"                            # unix ts override (0 => use block.timestamp)
//!
//! Flow:
//!   1) Find deploy blocks of OTCIndex + OTCCustody on Base, fork Anvil from max block.
//!   2) Wait for a real Deposit (from your JS client) on the forked node.
//!   3) Build Merkle root (alloy-merkle-tree) over the deposit intent; sign root (Alloy signer).
//!   4) Call OTCCustody.custodyToAddress(...) with VerificationData (may revert — OK for demo).
//!   5) Impersonate OTCCustody and call IOTCIndex.mint(...).
//!   6) Confirm Mint event -> logs "✅ Mint ..." and exit.

use std::{
    process::{Command, Stdio},
    str::FromStr,
    time::Duration,
};

use anyhow::{anyhow, Context, Result};
use dotenvy::dotenv;
use reqwest::Url;
use serde_json::json;
use tokio::time::sleep;
use tracing::{info, warn};
use tracing_subscriber::EnvFilter;

use alloy::{dyn_abi::DynSolValue, signers::Signer};
use alloy::{network::BlockResponse, primitives::LogData};
use alloy::{
    primitives::{keccak256, Address, Bytes, Log as PrimLog, B256, U256},
    providers::{ext::AnvilApi, Provider, ProviderBuilder, RootProvider},
    rpc::types::{BlockId, BlockNumberOrTag, Filter, FilterBlockOption, TransactionRequest},
    signers::local::PrivateKeySigner,
    sol,
    sol_types::{SolCall, SolEvent},
};
use alloy_consensus::BlockHeader;
use alloy_merkle_tree::standard_binary_tree::StandardMerkleTree;
use alloy_network::{ReceiptResponse, TransactionBuilder};
sol! {
    #[sol(rpc)]
    interface IOTCIndex {
        // events
        event Deposit(uint256 amount, address from, bytes quote, address affiliate);
        event Withdraw(uint256 amount, address to, bytes executionReport);
        event Mint(uint256 amount, address to, uint256 executionPrice, uint256 executionTime, address frontend);

        // funcs we use
        function getCollateralToken() external view returns (address);
        function mint(address target, uint256 amount, uint256 executionPrice, uint256 executionTime, address frontend) external;
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

/// Mirrors OTCCustody.VerificationData layout you provided.
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
        .arg(anvil_http.split(':').last().unwrap_or("8545")) // keep default if ANVIL_HTTP not set
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .context("failed to spawn anvil")?;

    // wait for JSON-RPC to come up
    wait_for_rpc(&anvil_http).await?;
    info!("Anvil fork online at {}", anvil_http);

    let signer: PrivateKeySigner = std::env::var("OPERATOR_PK")?.parse()?;
    let provider = ProviderBuilder::new()
        .wallet(signer)
        .connect("http://127.0.0.1:8545")
        .await?;

    // (optional) enable auto-impersonation on the fork
    provider.anvil_auto_impersonate_account(true).await?;
    // contracts
    let index = IOTCIndex::new(index_addr, provider.clone());
    let custody = IOTCCustody::new(custody_addr, provider.clone());
    let collateral_fb = index.getCollateralToken().call().await?.0;
    let collateral: Address = collateral_fb.into();

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
        wait_for_deposit(&provider, &filter, index_addr, poll_ms).await?;
    info!(
        "🧾 Deposit: amount={} from={} affiliate={} (block #{})",
        amount,
        fmt(from),
        fmt(affiliate),
        seen_at
    );

    // ---- 4) Merkle + signing (Alloy-only) ----
    // We make a leaf using a deterministic preimage. Adjust `leaf_bytes(...)` to match VerificationUtils format if needed.
    let leaf = DynSolValue::Tuple(vec![
        DynSolValue::Address(index_addr),
        DynSolValue::Address(from),
        DynSolValue::Uint(amount, 256),
        // pack quote -> keccak -> FixedBytes(32)
        DynSolValue::FixedBytes(keccak256(&quote.0), 32),
        DynSolValue::Address(affiliate),
    ]);

    // Build the OZ-style standard tree (sorted)
    let tree = StandardMerkleTree::of_sorted(&[leaf.clone()]);

    // Root and proof
    let root: B256 = tree.root();
    let root_b256: B256 = tree.root();
    let proof: Vec<B256> = tree
        .get_proof(&leaf)
        .map_err(|e| anyhow::anyhow!("get_proof failed"))?;

    // Optional local verify
    assert!(tree.verify_proof(&leaf, proof.clone()));

    // Alloy signing (EIP-191 style hash; here we just sign the root directly)
    let sig = operator_pk.sign_hash(&root_b256).await?;
    let (sig_e, sig_s) = split_sig_to_e_s(&sig.as_bytes());

    // ---- 5) Try calling custodyToAddress (may revert if Merkle rules differ; OK) ----
    // We craft a VerificationData that passes basic guards (timestamp <= now, new nullifier).
    let now: U256 = provider
        .get_block_by_number(BlockNumberOrTag::Latest)
        .await?
        .map(|b| U256::from(b.header().timestamp()))
        .unwrap_or(U256::ZERO);

    // OTCCustody state check requires v.state == custodyState[v.id]
    // We set id = root, so initCA(root) would have CA[root] = root; but custodyState[root] defaults to 0.
    // Therefore we send state=0. If your real flow sets a different state, change here.
    let v = VerificationData {
        id: root_b256,
        state: 0u8.into(),
        timestamp: U256::from(now), // <= block.timestamp passes checkExpiry
        pubKey: SchnorrCAKey {
            parity: 0u8.into(),
            x: B256::ZERO,
        },
        sig: SchnorrSignature { e: sig_e, s: sig_s },
        merkleProof: proof.into_iter().map(Into::into).collect(),
    };

    // We also need custody to have enough balance for `amount`. Since we can't rely on your index's deposit path
    // to have topped up custody, you can uncomment the funding block below to top-up via addressToCustody
    // (requires the caller to hold collateral and approve OTCCustody).
    //
    //  provider.anvil_impersonate_account(from).await?;
    //  approve + call `addressToCustody(v.id, collateral, amount)` here if desired.

    let c2a = IOTCCustody::custodyToAddressCall {
        token: collateral,
        destination: from,
        amount,
        v,
    }
    .abi_encode();

    let try_tx = TransactionRequest::default()
        .to(custody_addr)
        .with_from(Address::ZERO) // any; anvil accepts
        .with_input(Bytes::from(c2a))
        .with_gas_limit(1_500_000u64);

    match provider.eth_send_transaction_sync(try_tx).await {
        Ok(r) if r.status() => info!("✅ custodyToAddress tx sent: {:?}", r.transaction_hash()),
        Ok(r) => warn!("custodyToAddress reverted (expected in demo unless leaf encoding matches VerificationUtils). Receipt: {:?}", r),
        Err(e) => warn!("custodyToAddress call error (ok for demo): {e:#}"),
    }

    // ---- 6) Impersonate OTCCustody and call mint() ----
    provider.anvil_impersonate_account(custody_addr).await?;
    let exec_time = if exec_time_override.is_zero() {
        U256::from(now)
    } else {
        exec_time_override
    };

    let mint_call = IOTCIndex::mintCall {
        target: from,
        amount,
        executionPrice: exec_price,
        executionTime: exec_time,
        frontend,
    }
    .abi_encode();

    let mint_tx = TransactionRequest::default()
        .from(custody_addr)
        .to(index_addr)
        .with_input(Bytes::from(mint_call))
        .with_gas_limit(1_000_000u64);

    let mint_rcpt = provider.eth_send_transaction_sync(mint_tx).await?;
    if !mint_rcpt.status() {
        return Err(anyhow!(
            "mint() reverted — check onlyOTCCustody gating/impersonation"
        ));
    }
    info!("🎉 mint() ok. tx={:?}", mint_rcpt.transaction_hash());

    // Confirm Mint event
    let mint_logs = provider
        .get_logs(
            &Filter::new()
                .address(index_addr)
                .event("Mint(uint256,address,uint256,uint256,address)")
                .select(FilterBlockOption::Range {
                    from_block: Some(BlockNumberOrTag::Number(
                        mint_rcpt.block_number().unwrap_or(start_block),
                    )),
                    to_block: Some(BlockNumberOrTag::Latest),
                }),
        )
        .await?;
    for l in mint_logs {
        if let Ok(ev) = IOTCIndex::Mint::decode_log(&l.inner) {
            // `ev` is the typed event struct
            info!(
                "✅ Mint: amount={} to={} price={} time={} frontend={}",
                ev.amount,
                fmt(ev.to),
                ev.executionPrice,
                ev.executionTime,
                fmt(ev.frontend)
            );
        }
    }

    tracing::info!("Done. Your JS client should have observed the Mint event.");
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

/// Build the deposit "leaf" bytes. Adjust this to your on-chain VerificationUtils preimage format.
fn leaf_bytes(
    index: Address,
    from: Address,
    amount: U256,
    quote: &Bytes,
    affiliate: Address,
) -> Vec<u8> {
    // Example packed encoding:
    // keccak( abi.encodePacked("deposit", index, from, amount, keccak(quote), affiliate) )
    let mut out = Vec::new();
    out.extend_from_slice(b"deposit");
    out.extend_from_slice(index.as_slice());
    out.extend_from_slice(from.as_slice());
    let amt_be: [u8; 32] = amount.to_be_bytes();
    out.extend_from_slice(&amt_be);
    let qh = keccak256(&quote.0);
    out.extend_from_slice(qh.as_slice());
    out.extend_from_slice(affiliate.as_slice());
    out
}

/// Waits for the very first Deposit and returns its fields + the block number it was seen at.
async fn wait_for_deposit<P: Provider>(
    provider: &P,
    base_filter: &Filter,
    index_addr: Address,
    poll_ms: u64,
) -> Result<(U256, Address, Bytes, Address, u64)> {
    let mut last_from = current_block(provider).await?;
    loop {
        let tip = current_block(provider).await?;
        if tip > last_from {
            let range = Filter::new()
                .address(index_addr) // <- no unwrap()
                .event("Deposit(uint256,address,bytes,address)")
                .select(FilterBlockOption::Range {
                    from_block: Some(BlockNumberOrTag::Number(last_from + 1)),
                    to_block: Some(BlockNumberOrTag::Number(tip)),
                });

            let logs = provider.get_logs(&range).await?;
            for l in logs {
                if let Ok(ev) = IOTCIndex::Deposit::decode_log(&l.inner) {
                    let bn = l.block_number.unwrap_or(tip);
                    return Ok((ev.amount, ev.from, ev.quote.clone(), ev.affiliate, bn));
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

/// Binary-search the first block where code exists for `addr`.
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

fn split_sig_to_e_s(sig: &[u8]) -> (B256, B256) {
    // Alloy local signer returns 65-byte sig (r,s,v).
    // For demo, derive 'e' from keccak(r||s||v) and 's' from s (32b).
    // Your Schnorr scheme will differ — replace accordingly.
    if sig.len() < 65 {
        return (B256::ZERO, B256::ZERO);
    }
    let r = &sig[0..32];
    let s = &sig[32..64];
    let v = &sig[64..65];
    let mut rs = Vec::with_capacity(65);
    rs.extend_from_slice(r);
    rs.extend_from_slice(s);
    rs.extend_from_slice(v);
    let e = keccak256(&rs);
    (e, B256::from_slice(s))
}

fn fmt(a: Address) -> String {
    format!("{:#x}", a)
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
