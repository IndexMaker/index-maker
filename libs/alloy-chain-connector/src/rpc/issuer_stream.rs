use std::{
    collections::{HashMap, HashSet},
    f32::MIN,
    sync::Arc,
};

use alloy::{providers::Provider, sol_types::SolEvent};
use alloy_primitives::{keccak256, U256};
use alloy_rpc_types_eth::{BlockNumberOrTag, Filter, FilterBlockOption};
use chrono::Utc;
use eyre::{eyre, Context, OptionExt};
use index_core::blockchain::chain_connector::ChainNotification;
use itertools::Itertools;
use parking_lot::RwLock as AtomicLock;
use symm_core::core::{
    async_loop::AsyncLoop,
    bits::Address,
    functional::{PublishSingle, SingleObserver},
};
use tokio::time::sleep;

use crate::{credentials::MultiProvider, util::amount_converter::AmountConverter};
use otc_custody::{
    contracts::IOTCIndex::{Deposit, Mint, Withdraw},
    custody_client::CustodyClientMethods,
    index::index::IndexInstance,
};

pub struct RpcIssuerStream<P>
where
    P: Provider + Clone + 'static,
{
    providers: Option<MultiProvider<P>>,
    account_name: String,
    subscription_loop: AsyncLoop<eyre::Result<MultiProvider<P>>>,
}

impl<P> RpcIssuerStream<P>
where
    P: Provider + Clone + 'static,
{
    pub fn new(account_name: String, providers: MultiProvider<P>) -> Self {
        Self {
            providers: Some(providers),
            account_name,
            subscription_loop: AsyncLoop::new(),
        }
    }

    pub async fn unsubscribe(&mut self) -> eyre::Result<()> {
        let providers = self
            .subscription_loop
            .stop()
            .await
            .map_err(|err| eyre!("Failed to unsubscribe: {:?}", err))??;

        self.providers.replace(providers);
        Ok(())
    }

    pub async fn subscribe(
        &mut self,
        chain_id: u32,
        indexes_by_address: Arc<AtomicLock<HashMap<Address, Arc<IndexInstance>>>>,
        observer: Arc<AtomicLock<SingleObserver<ChainNotification>>>,
    ) -> eyre::Result<()> {
        let providers = self.providers.take().ok_or_eyre("Already subscribed")?;
        let mut providers_clone = providers.clone();

        let event_filter = Filter::new()
            .address(indexes_by_address.read().keys().cloned().collect_vec())
            .events(vec![
                Deposit::SIGNATURE,
                Withdraw::SIGNATURE,
                Mint::SIGNATURE,
            ]);

        let (poll_interval, backoff_period, max_failure_count) = providers
            .with_shared_date(|s| (s.poll_interval, s.poll_backoff_period, s.max_poll_failures));

        let (provider, rpc_url) = providers.current().ok_or_eyre("No providers")?;

        let mut last_block_from = provider
            .get_block_number()
            .await
            .map_err(|err| eyre!("Failed to fetch last block via: {:?}", rpc_url))?;

        let account_name = self.account_name.clone();
        let account_name_clone = account_name.clone();

        let mut poll_log_events_fn = async move |provider: &P, rpc_url: &String| -> eyre::Result<()> {
            let most_recent_block = provider
                .get_block_number()
                .await
                .context("Failed to obtain most recent block number")?;

            if most_recent_block > last_block_from {
                tracing::info!(
                    account_name = %account_name_clone,
                    %rpc_url,
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
                            %rpc_url,
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
            .start(async move |cancel_token| -> eyre::Result<MultiProvider<P>> {
                tracing::info!(%account_name, "Issuer stream polling loop started");
                let mut failure_count = 0;
                loop {
                    tokio::select! {
                        _ = cancel_token.cancelled() => {
                            break;
                        },
                        _ = tokio::time::sleep(poll_interval) => {
                            let (provider, rpc_url) = providers_clone
                                .next_provider()
                                .await
                                .current()
                                .ok_or_eyre("No provider")?;

                            if let Err(err) = poll_log_events_fn(provider, rpc_url).await {
                                tracing::warn!(%account_name, %rpc_url, "Polling log events failed: {:?}", err);
                                failure_count += 1;
                                if failure_count > max_failure_count {
                                    sleep(backoff_period).await;
                                    failure_count = 0;
                                }
                            }
                        }
                    }
                }
                tracing::info!(%account_name, "⚠️ Issuer stream polling loop exited");
                Ok(providers_clone)
            });

        Ok(())
    }
}
