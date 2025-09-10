use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};

use alloy::providers::Provider;
use alloy_primitives::{keccak256, U256};
use alloy_rpc_types_eth::{BlockNumberOrTag, Filter, FilterBlockOption};
use chrono::Utc;
use eyre::{eyre, Context, OptionExt};
use index_core::blockchain::chain_connector::ChainNotification;
use parking_lot::RwLock as AtomicLock;
use symm_core::core::{
    async_loop::AsyncLoop,
    bits::Address,
    functional::{PublishSingle, SingleObserver},
};

use crate::util::amount_converter::AmountConverter;
use otc_custody::{
    contracts::{
        IOTCIndex::{Deposit, Withdraw},
        ERC20,
    },
    custody_client::CustodyClientMethods,
    index::index::IndexInstance,
};

pub struct RpcIssuerStream<P>
where
    P: Provider + Clone + 'static,
{
    provider: Option<P>,
    account_name: String,
    subscription_loop: AsyncLoop<eyre::Result<P>>,
}

impl<P> RpcIssuerStream<P>
where
    P: Provider + Clone + 'static,
{
    pub fn new(account_name: String, provider: P) -> Self {
        Self {
            provider: Some(provider),
            account_name,
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
        indexes_by_address: Arc<AtomicLock<HashMap<Address, Arc<IndexInstance>>>>,
        observer: Arc<AtomicLock<SingleObserver<ChainNotification>>>,
    ) -> eyre::Result<()> {
        let provider = self.provider.take().ok_or_eyre("Already subscribed")?;
        let provider_clone = provider.clone();

        let event_filter = Filter::new().events(vec![
            b"Deposit(uint256,address,uint256,address,address)" as &[u8],
            b"Withdraw(uint256,address,bytes)" as &[u8],
        ]);

        let poll_interval = std::time::Duration::from_secs(10);
        let max_failure_count = 10;

        let mut last_block_from = provider.get_block_number().await?;

        let account_name = self.account_name.clone();
        let account_name_clone = account_name.clone();

        let mut poll_log_events_fn = async move || -> eyre::Result<()> {
            let most_recent_block = provider
                .get_block_number()
                .await
                .context("Failed to obtain most recent block number")?;

            if most_recent_block > last_block_from {
                tracing::info!(
                    account_name = %account_name_clone,
                    %last_block_from,
                    %most_recent_block, "⏱ Polling events");

                let range = event_filter.clone().select(FilterBlockOption::Range {
                    from_block: Some(BlockNumberOrTag::Number(last_block_from + 1)),
                    to_block: Some(BlockNumberOrTag::Number(most_recent_block)),
                });
                last_block_from = most_recent_block;

                let logs = provider.get_logs(&range).await?;
                for log_event in logs {
                    if let Ok(deposit_event) = log_event.log_decode::<Deposit>() {
                        let deposit_data = deposit_event.inner;
                        tracing::info!(
                            account_name = %account_name_clone,
                            "📥 Deposit: amount={} from={:#x} seq={} aff1={:#x} aff2={:#x}",
                            deposit_data.amount,
                            deposit_data.from,
                            deposit_data.seqNumNewOrderSingle,
                            deposit_data.affiliate1,
                            deposit_data.affiliate2
                        );

                        let decimals = indexes_by_address
                            .read()
                            .get(&deposit_data.address)
                            .ok_or_else(|| {
                                eyre!("Failed to find index by address: {}", deposit_data.address)
                            })?
                            .get_collateral_token_precision();

                        let converter = AmountConverter::new(decimals);
                        let amount = converter.into_amount(deposit_data.amount)?;

                        observer.read().publish_single(ChainNotification::Deposit {
                            chain_id,
                            address: deposit_data.from,
                            seq_num: deposit_data.seqNumNewOrderSingle,
                            affiliate1: Some(deposit_data.affiliate1),
                            affiliate2: Some(deposit_data.affiliate2),
                            amount,
                            timestamp: Utc::now(),
                        });
                    } else if let Ok(_) = log_event.log_decode::<Withdraw>() {
                        tracing::info!("Withdraw event");
                    }
                }
            }
            Ok(())
        };

        self.subscription_loop
            .start(async move |cancel_token| -> eyre::Result<P> {
                tracing::info!(%account_name, "Issuer stream polling loop started");
                let mut failure_count = 0;
                loop {
                    tokio::select! {
                        _ = cancel_token.cancelled() => {
                            break;
                        },
                        _ = tokio::time::sleep(poll_interval) => {
                            if let Err(err) = poll_log_events_fn().await {
                                tracing::warn!(%account_name, "Polling log events failed: {:?}", err);
                                failure_count += 1;
                                if failure_count > max_failure_count {
                                    break;
                                }
                            }
                        }
                    }
                }
                tracing::info!(%account_name, "⚠️ Issuer stream polling loop exited");
                Ok(provider_clone)
            });

        Ok(())
    }
}
