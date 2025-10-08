use std::{collections::HashMap, sync::Arc};

use alloy::{providers::Provider, sol_types::SolEvent};
use alloy_rpc_types_eth::{BlockNumberOrTag, Filter, FilterBlockOption, Log};
use chrono::Utc;
use eyre::{eyre, Context, OptionExt};
use futures::{future::join_all, stream::select_all, StreamExt};
use index_core::blockchain::chain_connector::ChainNotification;
use itertools::Itertools;
use parking_lot::RwLock as AtomicLock;
use symm_core::core::{
    async_loop::AsyncLoop,
    bits::{Address, Symbol},
    functional::{PublishSingle, SingleObserver},
};
use tokio::sync::mpsc::UnboundedReceiver;
use tokio_util::time::FutureExt;

use crate::{multiprovider::MultiProvider, util::amount_converter::AmountConverter};
use otc_custody::{
    contracts::IOTCIndex::{self, Deposit, Mint, Withdraw},
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

    pub async fn subscribe_polling(
        &mut self,
        chain_id: u32,
        mut poll_rx: UnboundedReceiver<(u32, Address, Symbol)>,
        indexes_by_address: Arc<AtomicLock<HashMap<Address, Arc<IndexInstance>>>>,
        observer: Arc<AtomicLock<SingleObserver<ChainNotification>>>,
    ) -> eyre::Result<()> {
        let mut providers = self.providers.take().ok_or_eyre("Already subscribed")?;
        let mut providers_clone = providers.clone();

        let event_filter = Filter::new()
            .address(indexes_by_address.read().keys().cloned().collect_vec())
            .events(vec![
                Deposit::SIGNATURE,
                Withdraw::SIGNATURE,
                Mint::SIGNATURE,
            ]);

        let timeout_period = std::time::Duration::from_secs(3);

        let mut last_block_from = providers
            .try_execute(async move |provider, rpc_url| -> eyre::Result<u64> {
                tracing::info!(%rpc_url, "Getting last block number...");
                provider
                    .get_block_number()
                    .timeout(timeout_period)
                    .await
                    .context("Failed to obtain most recent block number")?
                    .context("Failed to obtain most recent block number: Timeout")
            })
            .await?;

        tracing::info!("Last block number {}", last_block_from);

        let account_name = self.account_name.clone();
        let account_name_clone = account_name.clone();

        let mut poll_log_events_fn =
            async move |provider: &P, rpc_url: &String| -> eyre::Result<()> {
                let most_recent_block = provider
                    .get_block_number()
                    .timeout(timeout_period)
                    .await
                    .context("Failed to obtain most recent block number")?
                    .context("Failed to obtain most recent block number: Timeout")?;

                // limit the range
                last_block_from = last_block_from.max(most_recent_block - 10_000);

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

                    let logs = provider
                        .get_logs(&range)
                        .timeout(timeout_period)
                        .await
                        .context("Failed to obtain logs")?
                        .context("Failed to obtain logs: Timeout")?;

                    for log in logs {
                        Self::handle_log(
                            &account_name_clone,
                            chain_id,
                            &rpc_url,
                            log,
                            &indexes_by_address,
                            &observer,
                        )?;
                    }
                }
                Ok(())
            };

        self.subscription_loop
            .start(async move |cancel_token| -> eyre::Result<MultiProvider<P>> {
                tracing::info!(%account_name, "Issuer stream polling loop started");
                loop {
                    tokio::select! {
                        _ = cancel_token.cancelled() => {
                            break;
                        },
                        Some(_) = poll_rx.recv() => {
                            let (provider, rpc_url) = providers_clone
                                .next_provider()
                                .current()
                                .ok_or_eyre("No provider")?;

                            tracing::info!("Polling next events");
                            if let Err(err) = poll_log_events_fn(provider, rpc_url).await {
                                tracing::warn!(%account_name, %rpc_url, "⚠️ Polling log events failed: {:?}", err);
                            }
                        }
                    }
                }
                tracing::info!(%account_name, "Issuer stream polling loop exited");
                Ok(providers_clone)
            });

        Ok(())
    }

    pub async fn subscribe_streaming(
        &mut self,
        chain_id: u32,
        indexes_by_address: Arc<AtomicLock<HashMap<Address, Arc<IndexInstance>>>>,
        observer: Arc<AtomicLock<SingleObserver<ChainNotification>>>,
    ) -> eyre::Result<()> {
        let providers = self.providers.take().ok_or_eyre("Already subscribed")?;
        let account_name = self.account_name.clone();

        let event_filter = Filter::new()
            .address(indexes_by_address.read().keys().cloned().collect_vec())
            .events(vec![
                Deposit::SIGNATURE,
                Withdraw::SIGNATURE,
                Mint::SIGNATURE,
            ]);

        let subs = providers
            .get_providers()
            .iter()
            .map(async |(p, u)| (p.subscribe_logs(&event_filter).await, u))
            .collect_vec();

        let (subs, errors): (Vec<_>, Vec<_>) = join_all(subs)
            .await
            .into_iter()
            .map(|(s, u)| s.map(|s| (s, u)))
            .partition_result();

        if !errors.is_empty() {
            tracing::warn!(
                "⚠️ Failed to subscribe to stream(s): {}",
                errors.into_iter().map(|e| format!("{:?}", e)).join("; ")
            );
        }

        if subs.is_empty() {
            Err(eyre!("No subscriptions"))?
        }

        let rpc_urls = subs.iter().map(|(_, u)| u).join(",");

        let streams = subs
            .into_iter()
            .map(|(s, u)| {
                let u = u.clone();
                s.into_stream().map(move |x| (x, u.clone())).fuse()
            })
            .collect_vec();

        let mut sel = select_all(streams);
        tracing::info!(%rpc_urls, "✅ Subscriptions created");

        self.subscription_loop.start(
            async move |cancel_token| -> eyre::Result<MultiProvider<P>> {
                tracing::info!(%account_name, "🏎️ Issuer streaming loop started");

                loop {
                    tokio::select! {
                        _ = cancel_token.cancelled() => {
                            break;
                        },
                        maybe_log = sel.next() => {
                            match maybe_log {
                                Some((log, rpc_url)) => {
                                    if let Err(err) = Self::handle_log(
                                        &account_name,
                                        chain_id,
                                        &rpc_url,
                                        log,
                                        &indexes_by_address,
                                        &observer,
                                    ) {
                                        tracing::info!(
                                            %account_name,
                                            %rpc_url,
                                            "⚠️ Failed to handle log: {:?}", err);
                                    }
                                },
                                None => {
                                    tracing::warn!(%account_name, "⚠️ All streams closed");
                                    break;
                                }
                            }
                        },
                    }
                }

                tracing::info!(%account_name, "Issuer streaming loop exited");
                Ok(providers)
            },
        );

        Ok(())
    }

    fn handle_log(
        account_name: &str,
        chain_id: u32,
        rpc_url: &str,
        log: Log,
        indexes_by_address: &Arc<AtomicLock<HashMap<Address, Arc<IndexInstance>>>>,
        observer: &Arc<AtomicLock<SingleObserver<ChainNotification>>>,
    ) -> eyre::Result<()> {
        match log.topic0() {
            Some(&IOTCIndex::Deposit::SIGNATURE_HASH) => {
                if let Ok(deposit_event) = log.log_decode::<Deposit>() {
                    let deposit_data = deposit_event.inner;
                    tracing::info!(
                        account_name = %account_name,
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
                } else {
                    tracing::warn!("Failed to parse Deposit event");
                }
            }
            Some(&IOTCIndex::Withdraw::SIGNATURE_HASH) => {}
            Some(&IOTCIndex::Mint::SIGNATURE_HASH) => {}
            _ => (),
        }
        Ok(())
    }
}
