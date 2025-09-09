use alloy::{
    hex,
    network::EthereumWallet,
    providers::{Provider, ProviderBuilder},
    signers::local::{LocalSigner, PrivateKeySigner},
};
use chrono::Utc;
use eyre::OptionExt;
use eyre::Result;
use parking_lot::RwLock as AtomicLock;
use safe_math::safe;
use serde::de::IntoDeserializer;
use std::{collections::HashSet, sync::Arc, time::Duration};
use symm_core::core::functional::{PublishSingle, SingleObserver};
use symm_core::core::{async_loop::AsyncLoop, bits::Amount, decimal_ext::DecimalExt};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use tokio_util::sync::CancellationToken;

use crate::across_deposit::AcrossDepositBuilder;
use crate::commands::{ChainCommand, ChainOperationResult};
use crate::config::EvmConnectorConfig;
use crate::contracts::ERC20;
use crate::utils::{calculate_gas_fee_usdc, IntoEvmAmount};
use index_core::blockchain::chain_connector::ChainNotification;
use symm_core::core::bits::{Address as CoreAddress, Symbol};

use crate::utils::IntoAmount;
use alloy_primitives::{keccak256, Address, U256};
use alloy_rpc_types_eth::{Filter, Log};
use futures::{task::ArcWake, StreamExt};
use std::str::FromStr;
/// Individual chain operation worker
/// Handles blockchain operations for a specific chain
pub struct ChainOperation {
    chain_id: u32,
    rpc_url: String,
    private_key: String,
    operation_loop: AsyncLoop<Result<()>>,
    result_sender: UnboundedSender<ChainOperationResult>,
    /// Observer for publishing ChainNotification events to solver
    chain_observer: Arc<AtomicLock<SingleObserver<ChainNotification>>>,
}

impl ChainOperation {
    pub fn new(
        chain_id: u32,
        rpc_url: String,
        private_key: String,
        result_sender: UnboundedSender<ChainOperationResult>,
        chain_observer: Arc<AtomicLock<SingleObserver<ChainNotification>>>,
    ) -> Self {
        Self {
            chain_id,
            rpc_url,
            private_key,
            operation_loop: AsyncLoop::new(),
            result_sender,
            chain_observer,
        }
    }

    pub fn start(&mut self, command_receiver: UnboundedReceiver<ChainCommand>) -> Result<()> {
        let chain_id = self.chain_id;
        let rpc_url = self.rpc_url.clone();
        let private_key = self.private_key.clone();
        let result_sender = self.result_sender.clone();
        let chain_observer = self.chain_observer.clone();

        self.operation_loop.start(async move |cancel_token| {
            match Self::operation_loop(
                chain_id,
                rpc_url,
                private_key,
                command_receiver,
                result_sender,
                chain_observer.clone(),
                cancel_token,
            )
            .await
            {
                Err(err) => {
                    tracing::warn!("Failed to start chain operation loop: {:?}", err);
                    chain_observer
                        .read()
                        .publish_single(ChainNotification::ChainDisconnected {
                            chain_id,
                            reason: format!("{}", err),
                            timestamp: Utc::now(),
                        });
                    Err(err)
                }
                Ok(()) => Ok(()),
            }
        });

        Ok(())
    }

    pub async fn stop(&mut self) -> Result<()> {
        match self.operation_loop.stop().await {
            Ok(result) => result,
            Err(_) => Ok(()), // Handle join error gracefully
        }
    }

    pub async fn operation_loop(
        chain_id: u32,
        rpc_url: String,
        private_key: String,
        mut command_receiver: UnboundedReceiver<ChainCommand>,
        result_sender: UnboundedSender<ChainOperationResult>,
        chain_observer: Arc<AtomicLock<SingleObserver<ChainNotification>>>,
        cancel_token: CancellationToken,
    ) -> Result<()> {
        tracing::info!(%chain_id, %rpc_url, "Chain operation loop started (HTTP polling)");

        // ---- Provider & wallet (HTTP) ----
        let wallet = private_key.parse::<PrivateKeySigner>()?;
        let provider = ProviderBuilder::new()
            .wallet(wallet.clone())
            .connect(&rpc_url)
            .await?;

        // ---- Optional: init AcrossDepositBuilder ----
        let deposit_builder = AcrossDepositBuilder::new(provider.clone(), wallet.address())
            .await
            .ok();

        // ---- Notify connected ----
        {
            let observer = chain_observer.read();
            observer.publish_single(ChainNotification::ChainConnected {
                chain_id,
                timestamp: Utc::now(),
            });
        }

        // ---- Event signatures / filter scaffold ----
        let deposit_sig = keccak256("Deposit(address,uint256,uint256)".as_bytes());
        let withdraw_sig = keccak256("Withdraw(uint256,address,bytes)".as_bytes());

        // Base filter (we'll add from/to block per poll)
        // Keep your existing multi-event builder; we’ll clone and bound it per query.
        let base_filter = Filter::new().events(vec![
            b"Deposit(address,uint256,uint256)" as &[u8],
            b"Withdraw(uint256,address,bytes)" as &[u8],
        ]);

        let poll_interval = Duration::from_secs(3);
        let startup_lookback: u64 = std::env::var("LOG_STARTUP_LOOKBACK")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(3); // re-scan a few recent blocks on startup

        // Establish block cursor (with a small lookback to avoid missing recent events)
        let mut next_block = provider
            .get_block_number()
            .await?
            .saturating_sub(startup_lookback.into());

        loop {
            tokio::select! {
                _ = cancel_token.cancelled() => break,

                _ = tokio::time::sleep(poll_interval) => {
                    let tip = provider.get_block_number().await?;
                    if tip >= next_block {
                        let range_filter = base_filter.clone()
                            .from_block(next_block)
                            .to_block(tip);

                        match provider.get_logs(&range_filter).await {
                            Ok(logs) => {
                                for log_entry in logs {
                                    tracing::debug!(%chain_id, "HTTP poll: got log");
                                    let mut seen: HashSet<(alloy_primitives::B256, u64)> = HashSet::new();
                                    let Some(txh) = log_entry.transaction_hash else { continue; };
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
                                            tracing::warn!("Deposit log missing indexed sender in topics[1]");
                                            continue;
                                        }
                                        if data.len() < 64 {
                                            tracing::warn!("Malformed deposit log: expected 64 bytes, got {}", data.len());
                                            continue;
                                        }

                                        // sender = last 20 bytes of topics[1]
                                        let t1 = topics[1].as_slice();
                                        let sender = Address::from_slice(&t1[12..32]);

                                        let dst_chain_id = U256::from_be_slice(&data[0..32]);
                                        let amount_raw   = U256::from_be_slice(&data[32..64]);
                                        let amount       = amount_raw.into_amount_usdc()?;

                                        tracing::info!(
                                            "Deposit event: sender={} dst_chain_id={} amount={}",
                                            sender, dst_chain_id, amount
                                        );

                                        let observer = chain_observer.read();
                                        //observer.publish_single(ChainNotification::Deposit {
                                        //    chain_id,
                                        //    address: sender,
                                        //    amount,
                                        //    timestamp: Utc::now(),
                                        //});

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

                Some(command) = command_receiver.recv() => {
                    let result = Self::execute_command(
                        chain_id,
                        command,
                        &deposit_builder,
                        &provider,
                    ).await;

                    let operation_result = match result {
                        Ok(tx_hash) => ChainOperationResult::Success {
                            chain_id,
                            transaction_hash: tx_hash,
                            result_data: None,
                        },
                        Err(err) => {
                            tracing::warn!(?err, "Chain operation failure");
                            ChainOperationResult::Failure {
                                chain_id,
                                error: format!("{:?}", err),
                            }
                        },
                    };

                    if let Err(e) = result_sender.send(operation_result) {
                        tracing::error!("Failed to send operation result: {}", e);
                        break;
                    }
                }
            }
        }

        // --- Emit disconnected ---
        {
            let observer = chain_observer.read();
            observer.publish_single(ChainNotification::ChainDisconnected {
                chain_id,
                reason: "Chain operation loop exited successfully".into(),
                timestamp: Utc::now(),
            });
        }

        tracing::info!(%chain_id, "Chain operation loop stopped");
        Ok(())
    }

    async fn execute_command<P: Provider + Clone + 'static>(
        chain_id: u32,
        command: ChainCommand,
        deposit_builder: &Option<AcrossDepositBuilder<P>>,
        provider: &P,
    ) -> Result<Option<String>> {
        match command {
            // ERC20 transfer: requires approval first
            ChainCommand::Erc20Transfer {
                token_address,
                from,
                to,
                amount,
                cumulative_fee,
                callback,
                ..
            } => {
                tracing::info!(%token_address, %from, %to, %amount, %cumulative_fee, "Erc20 Transfer");

                // Create ERC20 contract instance
                let token_contract = ERC20::new(token_address, provider.clone());
                let transfer_amount = amount.into_evm_amount(6)?;

                let allowance = token_contract.allowance(from, to).call().await?;
                tracing::info!(%allowance, %from, %to, "Transfer allowance");

                match token_contract
                    .transferFrom(from, to, transfer_amount)
                    .send()
                    .await
                {
                    Ok(pending_tx) => {
                        match pending_tx.get_receipt().await {
                            Ok(receipt) => {
                                let tx_hash =
                                    format!("0x{}", hex::encode(receipt.transaction_hash));

                                // Calculate gas fee using simplified utility
                                let gas_used = receipt.gas_used.into_evm_amount(0)?;
                                let fee_amount = calculate_gas_fee_usdc(provider, gas_used).await?;

                                // Calculate net amounts
                                let updated_transfer_amount = safe!(amount - fee_amount)
                                    .ok_or_eyre("Failed to compute net transfer amount")?;
                                let updated_cumulative_fee = safe!(cumulative_fee + fee_amount)
                                    .ok_or_eyre("Failed to compute cumulative fee")?;

                                // Use Amount directly for display (now properly formatted)
                                tracing::info!(
                                    "ERC20: {} USDC (fee: {}) tx: {}",
                                    updated_transfer_amount,
                                    updated_cumulative_fee,
                                    tx_hash
                                );

                                // Call the callback with the updated amounts (net of gas fees)
                                if let Err(e) =
                                    callback(updated_transfer_amount, updated_cumulative_fee)
                                {
                                    tracing::error!("Error in ERC20 transfer callback: {}", e);
                                }

                                Ok(Some(tx_hash))
                            }
                            Err(e) => Err(eyre::eyre!("ERC20 transfer receipt failed: {}", e)),
                        }
                    }
                    Err(e) => Err(eyre::eyre!("ERC20 transfer failed: {}", e)),
                }
            }
            ChainCommand::MintIndex {
                symbol: _,
                quantity: _,
                recipient: _,
                execution_price: _,
                execution_time: _,
                ..
            } => {
                // TODO: Implement actual index minting logic
                // For now, simulate successful minting
                Ok(Some("0xmint_index...".to_string())) // Mock transaction hash
            }
            ChainCommand::BurnIndex {
                symbol: _,
                quantity: _,
                recipient: _,
                ..
            } => {
                // TODO: Implement actual index burning logic
                // For now, simulate successful burning
                Ok(Some("0xburn_index...".to_string())) // Mock transaction hash
            }
            ChainCommand::Withdraw {
                recipient: _,
                amount: _,
                execution_price: _,
                execution_time: _,
                ..
            } => {
                // TODO: Implement actual withdrawal logic
                // For now, simulate successful withdrawal
                Ok(Some("0xwithdraw...".to_string())) // Mock transaction hash
            }
            ChainCommand::ExecuteCompleteAcrossDeposit {
                from,
                to,
                deposit_amount,
                origin_chain_id,
                destination_chain_id,
                party: _,
                cumulative_fee,
                callback,
                ..
            } => {
                tracing::info!(
                    %from, %to, %deposit_amount, %origin_chain_id, %destination_chain_id, %cumulative_fee,
                    "Across Transfer");

                if let Some(builder) = deposit_builder {
                    // Use config USDC addresses: USDC_ARBITRUM_ADDRESS as input, USDC_BASE_ADDRESS as output
                    let config = EvmConnectorConfig::default();
                    let input_token = config.get_usdc_address("arbitrum").unwrap();
                    let output_token = config.get_usdc_address("base").unwrap();

                    match builder
                        .execute_complete_across_deposit(
                            from,
                            to,
                            input_token,
                            output_token,
                            deposit_amount.into_evm_amount_usdc()?,
                            origin_chain_id,
                            destination_chain_id,
                        )
                        .await
                    {
                        Ok(gas_used) => {
                            // Calculate gas fee using simplified utility
                            let fee_amount = calculate_gas_fee_usdc(provider, gas_used).await?;

                            // Calculate net amounts
                            let updated_deposit_amount = safe!(deposit_amount - fee_amount)
                                .ok_or_eyre("Failed to compute deposit amount")?;
                            let updated_cumulative_fee = safe!(cumulative_fee + fee_amount)
                                .ok_or_eyre("Failed to compute cumulative fee")?;

                            // Use Amount directly for display since it's already in human-readable format
                            tracing::info!(
                                "Across: {} USDC (fee: {})",
                                updated_deposit_amount,
                                updated_cumulative_fee
                            );

                            // Pass the updated amounts to the callback
                            callback(updated_deposit_amount, updated_cumulative_fee).map_err(
                                |err| {
                                    eyre::eyre!(
                                        "ExecuteCompleteAcrossDeposit callback failed {:?}",
                                        err
                                    )
                                },
                            )?;

                            Ok(Some("0xacross_complete...".to_string()))
                        }
                        Err(e) => Err(eyre::eyre!("ExecuteCompleteAcrossDeposit failed: {}", e)),
                    }
                } else {
                    Err(eyre::eyre!(
                        "AcrossDepositBuilder not initialized for chain {}",
                        chain_id
                    ))
                }
            }
        }
    }
}
