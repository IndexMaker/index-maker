use std::{
    collections::HashMap,
    sync::Arc,
    thread::{sleep, spawn},
    time::Duration,
};
use serde_json::json;
use symm_core::{
    core::{
        logging::log_init,
    },
    init_log,
};
use alloy::{
    primitives::{keccak256, Address, Bytes, B256, U256},
    providers::{Provider, ProviderBuilder},
    sol,
    transports::ws::WsConnect,
};
use alloy_evm_connector::evm_connector::EvmConnector;
use alloy_rpc_types_eth::Filter;
use chrono::Utc;
use crossbeam::{channel::unbounded, select};
use futures_util::StreamExt;
use index_core::{
    blockchain::chain_connector::{ChainConnector, ChainNotification},
    index::basket::Basket,
};
use itertools::Itertools;
use parking_lot::RwLock;
use rust_decimal::dec;
use symm_core::core::{
    bits::Amount,
    functional::{IntoObservableSingleFun},
};

sol! {
    event Deposit(address indexed account, uint256 chainId, uint256 amount);
}

fn u256_to_amount_usdc(v: U256) -> eyre::Result<Amount> {
    // Insert decimal point 6 places from the right to avoid precision loss.
    let s = v.to_string();
    let s_fmt = if s.len() <= 6 {
        format!("0.{:0>6}", s)
    } else {
        let (intp, frac) = s.split_at(s.len() - 6);
        format!("{}.{}", intp, frac)
    };
    Ok(s_fmt.parse::<Amount>()?)
}

pub fn handle_chain_event(event: &ChainNotification) {
    match event {
        ChainNotification::ChainConnected {
            chain_id,
            timestamp,
        } => {}
        ChainNotification::ChainDisconnected {
            chain_id,
            timestamp,
        } => {}
        ChainNotification::CuratorWeightsSet(symbol, basket_definition) => {
            println!(
                "(evm-connector-main) CuratorWeightsSet {}: {}",
                symbol,
                basket_definition
                    .weights
                    .iter()
                    .map(|w| format!("{}:{}", w.asset.ticker, w.weight))
                    .join(", ")
            );
        }
        ChainNotification::Deposit {
            chain_id,
            address,
            amount,
            timestamp,
        } => {
            println!(
                "(evm-connector-main) Deposit {} {} {} {}",
                chain_id, address, amount, timestamp
            );
        }
        ChainNotification::WithdrawalRequest {
            chain_id,
            address,
            amount,
            timestamp,
        } => {
            println!(
                "(evm-connector-main) WithdrawalRequest {} {} {} {}",
                chain_id, address, amount, timestamp
            );
        }
    }
}

#[tokio::main]
pub async fn main() {
    init_log!();

    let evm_connector = Arc::new(RwLock::new(EvmConnector::new()));

    let (event_tx, event_rx) = unbounded::<ChainNotification>();

    let evm_connector_weak = Arc::downgrade(&evm_connector);

    let handle_event_internal = move |e: ChainNotification| {
        let evm_connector = evm_connector_weak.upgrade().unwrap();
        match e {
            ChainNotification::ChainConnected {
                chain_id,
                timestamp,
            } => {
                tracing::info!(%chain_id, %timestamp, "Chain connected");
            }
            ChainNotification::ChainDisconnected {
                chain_id,
                timestamp,
            } => {
                tracing::info!(%chain_id, %timestamp, "Chain disconnected");
            }
            ChainNotification::CuratorWeightsSet(symbol, basket_definition) => {
                tracing::info!(
                    %symbol, basket_definition = %json!(basket_definition.weights),
                    "Curator weights set");

                // When we receive curator weights, we respond with solver weights set
                let individual_prices = HashMap::from_iter([
                    ("A1".into(), dec!(100000.0)),
                    ("A2".into(), dec!(1000.0)),
                    ("A3".into(), dec!(10.0)),
                    ("A4".into(), dec!(100.0)),
                ]);
                let basket = Arc::new(
                    Basket::new_with_prices(basket_definition, &individual_prices, dec!(1000.0))
                        .unwrap(),
                );
                evm_connector.write().solver_weights_set(symbol, basket);
            }
            ChainNotification::Deposit {
                chain_id,
                address,
                amount,
                timestamp,
            } => {
                tracing::info!(%chain_id, %address, %amount, %timestamp, "Deposit");
                // When we receive deposit, we respond with mint and burn
                evm_connector.write().mint_index(
                    chain_id,
                    "I1".into(),
                    dec!(2.0),
                    address,
                    amount,
                    timestamp,
                );
                evm_connector
                    .write()
                    .burn_index(chain_id, "I1".into(), dec!(1.0), address);
            }
            ChainNotification::WithdrawalRequest {
                chain_id,
                address,
                amount,
                timestamp,
            } => {
                tracing::info!(%chain_id, %address, %amount, %timestamp, "Withdrawal request");
                // When we receive withdrawal request, we respond with withdraw
                evm_connector
                    .write()
                    .withdraw(chain_id, address, amount, dec!(1000.0), timestamp);
            }
        }
    };

    // Clone the sender for the observer callback
    let tx_for_observer = event_tx.clone();
    evm_connector.write().set_observer_fn(move |e| {
        // still forward internal connector events
        tx_for_observer.send(e).unwrap();
    });

    // Start + connect
    {
        let mut conn = evm_connector.write();
        conn.start().expect("Failed to start EvmConnector");
    }
    {
        let mut conn = evm_connector.write();
        conn.connect_arbitrum()
            .await
            .expect("Failed to connect to ARBITRUM");
    }

    // ---------- On-chain Deposit subscription (3-arg event) ----------
    // Normalize DEFAULT_RPC to ws:// if http:// is provided
    let raw = std::env::var("DEFAULT_RPC").unwrap_or_else(|_| "ws://127.0.0.1:8545".to_string());
    let ws_url = if raw.starts_with("http://") {
        raw.replacen("http://", "ws://", 1)
    } else if raw.starts_with("https://") {
        raw.replacen("https://", "wss://", 1)
    } else {
        raw
    };

    // SAME address you use in the FE
    let deposit_addr: Address = "0x3d72617b8ef426fefd1ea0765684a51862304b78"
        .parse()
        .expect("invalid DepositEmitter address");

    // WS provider for logs
    let ws = WsConnect::new(ws_url);
    let provider = ProviderBuilder::new()
        .on_ws(ws)
        .await
        .expect("ws connect failed");

    // 3-arg event signature (NOTE: *no* 'indexed' in the signature string)
    let deposit3_sig: B256 = keccak256("Deposit(address,uint256,uint256)".as_bytes());

    // Optional other events
    let withdraw_sig: B256 = keccak256("Withdraw(uint256,address,bytes)".as_bytes());

    // Filter: contract + the 3-arg deposit (+ optional withdraw)
    let filter = alloy_rpc_types_eth::Filter::new()
        .address(deposit_addr)
        .events(vec![
            b"Deposit(address,uint256,uint256)" as &[u8],
            b"Withdraw(uint256,address,bytes)" as &[u8],
        ]);

    // Subscribe and turn into a Stream
    let mut sub = provider
        .subscribe_logs(&filter)
        .await
        .expect("subscribe_logs");
    let mut stream = sub.into_stream();

    // helper: U256 → Amount (USDC 6 decimals)
    let u256_to_amount_usdc = |v: U256| -> eyre::Result<Amount> {
        let s = v.to_string();
        let s_fmt = if s.len() <= 6 {
            format!("0.{:0>6}", s)
        } else {
            let (intp, frac) = s.split_at(s.len() - 6);
            format!("{}.{}", intp, frac)
        };
        Ok(s_fmt.parse::<Amount>()?)
    };

    // Clone sender for this async task
    let tx_for_logs = event_tx.clone();
    tokio::spawn(async move {
        while let Some(log_entry) = stream.next().await {
            // topic0 decides which event we got
            let Some(topic0) = log_entry.topic0().copied() else {
                tracing::warn!("log without topic0");
                continue;
            };
            let data: &[u8] = log_entry.inner.data.data.as_ref();

            if topic0 == deposit3_sig {
                // event Deposit(address indexed account, uint256 chainId, uint256 amount)
                // topics[1] = indexed account (left-padded 32)
                // data[0..32] = chainId, data[32..64] = amount
                let topics = log_entry.topics();
                if topics.len() < 2 || data.len() < 64 {
                    tracing::warn!("Malformed Deposit log (3-arg)");
                    continue;
                }

                // decode indexed account
                let acct_topic = topics[1].as_slice();
                let sender = Address::from_slice(&acct_topic[12..32]);

                // decode data
                let chain_id_raw = U256::from_be_slice(&data[0..32]);
                let amount_raw = U256::from_be_slice(&data[32..64]);

                let amount = match u256_to_amount_usdc(amount_raw) {
                    Ok(a) => a,
                    Err(e) => {
                        tracing::error!("amount convert failed: {e:?}");
                        continue;
                    }
                };

                let chain_id_u32: u32 = chain_id_raw.to_string().parse::<u64>().unwrap_or(0) as u32;

                let _ = tx_for_logs.send(ChainNotification::Deposit {
                    chain_id: chain_id_u32,
                    address: sender,
                    amount,
                    timestamp: Utc::now(),
                });

                tracing::info!(
                    "Deposit(3) account={:#x} chainId={} amount={}",
                    sender,
                    chain_id_u32,
                    amount
                );
            } else if topic0 == withdraw_sig {
                tracing::info!("Withdrawal event received ({} bytes)", data.len());
            } else {
                tracing::debug!("Unrecognized topic0: {topic0:?}");
            }
        }
    });

    // ---------- Existing crossbeam loop ----------

    spawn(move || {
        tracing::info!("Listening for EVM events...");
        loop {
            select! {
                recv(event_rx) -> res => {
                    handle_event_internal(res.unwrap());
                }
            }
            tracing::info!("Finished listening for EVM events.");
        }
    });

    // Keep process alive (adjust as needed)
    sleep(Duration::from_secs(600));

    // Proper shutdown
    tracing::info!("Shutting down EvmConnector...");
    evm_connector
        .write()
        .stop()
        .await
        .expect("Failed to stop EvmConnector");

    tracing::info!("EvmConnector shutdown complete");
}
