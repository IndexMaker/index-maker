use std::{collections::HashSet, sync::Arc};

use alloy::providers::Provider;
use alloy_primitives::{keccak256, U256};
use alloy_rpc_types_eth::Filter;
use chrono::Utc;
use eyre::{eyre, OptionExt};
use index_core::blockchain::chain_connector::ChainNotification;
use parking_lot::RwLock as AtomicLock;
use symm_core::core::{
    async_loop::AsyncLoop,
    bits::Address,
    functional::{PublishSingle, SingleObserver},
};

use crate::util::amount_converter::AmountConverter;
use ca_helper::contracts::ERC20;

pub struct RpcIssuerStream<P>
where
    P: Provider + Clone + 'static,
{
    provider: Option<P>,
    subscription_loop: AsyncLoop<eyre::Result<P>>,
}

impl<P> RpcIssuerStream<P>
where
    P: Provider + Clone + 'static,
{
    pub fn new(provider: P) -> Self {
        Self {
            provider: Some(provider),
            subscription_loop: AsyncLoop::new(),
        }
    }

    pub async fn unsubscribe(&mut self) -> eyre::Result<()> {
        let provider = self
            .subscription_loop
            .stop()
            .await
            .map_err(|err| eyre!("Failed to unsubscribe: {:?}", err))??;

        self.provider.replace(provider);
        Ok(())
    }

    pub async fn subscribe(
        &mut self,
        chain_id: u32,
        usdc_address: Address,
        observer: Arc<AtomicLock<SingleObserver<ChainNotification>>>,
    ) -> eyre::Result<()> {
        let provider = self.provider.take().ok_or_eyre("Already subscribed")?;

        let usdc = ERC20::new(usdc_address, provider.clone());
        let decimals = usdc.decimals().call().await?;
        let converter = AmountConverter::new(decimals);

        // ---- Event signatures / filter scaffold ----
        let deposit_sig = keccak256("Deposit(address,uint256,uint256)".as_bytes());
        let withdraw_sig = keccak256("Withdraw(uint256,address,bytes)".as_bytes());

        // Base filter (we'll add from/to block per poll)
        // Keep your existing multi-event builder; we’ll clone and bound it per query.
        let base_filter = Filter::new().events(vec![
            b"Deposit(address,uint256,uint256)" as &[u8],
            b"Withdraw(uint256,address,bytes)" as &[u8],
        ]);

        let poll_interval = std::time::Duration::from_secs(3);
        let startup_lookback: u64 = std::env::var("LOG_STARTUP_LOOKBACK")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(3); // re-scan a few recent blocks on startup

        // Establish block cursor (with a small lookback to avoid missing recent events)
        let mut next_block = provider
            .get_block_number()
            .await?
            .saturating_sub(startup_lookback.into());

        self.subscription_loop
            .start(async move |cancel_token| -> eyre::Result<P> {
                tracing::info!("");
                loop {
                    tokio::select! {
                        _ = cancel_token.cancelled() => {
                            break;
                        },
                        _ = tokio::time::sleep(poll_interval) => {

                            let tip = provider.get_block_number().await?;
                            if tip >= next_block {
                                let range_filter = base_filter.clone().from_block(next_block).to_block(tip);

                                match provider.get_logs(&range_filter).await {
                                    Ok(logs) => {
                                        for log_entry in logs {
                                            tracing::debug!(%chain_id, "HTTP poll: got log");

                                            let mut seen: HashSet<(alloy_primitives::B256, u64)> =
                                                HashSet::new();
                                            let Some(txh) = log_entry.transaction_hash else {
                                                continue;
                                            };
                                            let li = log_entry.log_index.unwrap_or_default(); // u64
                                            let key = (txh, li);

                                            if !log_entry.removed && !seen.insert(key) {
                                                continue;
                                            }

                                            // topic0 = signature
                                            let Some(topic0) = log_entry.topic0() else {
                                                tracing::warn!("Log without topic0");
                                                continue;
                                            };

                                            let ld = log_entry.data();
                                            let topics = ld.topics();
                                            let data: &[u8] = ld.data.as_ref();

                                            if *topic0 == deposit_sig {
                                                // public getters
                                                if topics.len() < 2 {
                                                    tracing::warn!(
                                                        "Deposit log missing indexed sender in topics[1]"
                                                    );
                                                    continue;
                                                }
                                                if data.len() < 64 {
                                                    tracing::warn!(
                                                        "Malformed deposit log: expected 64 bytes, got {}",
                                                        data.len()
                                                    );
                                                    continue;
                                                }

                                                // sender = last 20 bytes of topics[1]
                                                let t1 = topics[1].as_slice();
                                                let sender = Address::from_slice(&t1[12..32]);

                                                let dst_chain_id = U256::from_be_slice(&data[0..32]);
                                                let amount_raw = U256::from_be_slice(&data[32..64]);
                                                let amount = converter.into_amount(amount_raw)?;

                                                tracing::info!(
                                                    "Deposit event: sender={} dst_chain_id={} amount={}",
                                                    sender,
                                                    dst_chain_id,
                                                    amount
                                                );

                                                let observer = observer.read();
                                                observer.publish_single(ChainNotification::Deposit {
                                                    chain_id,
                                                    address: sender,
                                                    amount,
                                                    timestamp: Utc::now(),
                                                });
                                            } else if *topic0 == withdraw_sig {
                                                tracing::info!("Withdrawal event received: {:?}", data);
                                            } else {
                                                tracing::warn!("Unknown event signature: {:?}", topic0);
                                            }
                                        }

                                        // advance cursor past scanned tip
                                        next_block = tip.saturating_add(1u64.into());
                                    }
                                    Err(e) => {
                                        tracing::warn!("get_logs failed: {}", e);
                                    }
                                }
                            }
                        }
                    }
                }
                Ok(provider)
            });

        Ok(())
    }
}
