use alloy::{
    hex,
    providers::{Provider, ProviderBuilder},
    signers::local::LocalSigner,
};
use chrono::Utc;
use eyre::OptionExt;
use eyre::Result;
use parking_lot::RwLock as AtomicLock;
use safe_math::safe;
use std::str::FromStr;
use std::sync::Arc;
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
            Self::operation_loop(
                chain_id,
                rpc_url,
                private_key,
                command_receiver,
                result_sender,
                chain_observer,
                cancel_token,
            )
            .await
        });

        Ok(())
    }

    pub async fn stop(&mut self) -> Result<()> {
        match self.operation_loop.stop().await {
            Ok(result) => result,
            Err(_) => Ok(()), // Handle join error gracefully
        }
    }

    async fn operation_loop(
        chain_id: u32,
        rpc_url: String,
        private_key: String,
        mut command_receiver: UnboundedReceiver<ChainCommand>,
        result_sender: UnboundedSender<ChainOperationResult>,
        chain_observer: Arc<AtomicLock<SingleObserver<ChainNotification>>>,
        cancel_token: CancellationToken,
    ) -> Result<()> {
        // Initialize provider and contracts
        let wallet = LocalSigner::from_str(&private_key)?;
        let provider = ProviderBuilder::new()
            .wallet(wallet.clone())
            .connect_http(rpc_url.parse()?);

        // Initialize AcrossDepositBuilder for this chain
        let deposit_builder = AcrossDepositBuilder::new(provider.clone(), wallet.address())
            .await
            .ok();

        // Emit connected event
        {
            let observer = chain_observer.read();
            observer.publish_single(ChainNotification::ChainConnected {
                chain_id,
                timestamp: Utc::now(),
            });
        }

        loop {
            tokio::select! {
                _ = cancel_token.cancelled() => {
                    break;
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
                        Err(e) => ChainOperationResult::Failure {
                            chain_id,
                            error: e.to_string(),
                        },
                    };

                    if let Err(e) = result_sender.send(operation_result) {
                        tracing::error!("Failed to send operation result: {}", e);
                        break;
                    }
                }
            }
        }

        // Emit disconnected event
        {
            let observer = chain_observer.read();
            observer.publish_single(ChainNotification::ChainDisconnected {
                chain_id,
                timestamp: Utc::now(),
            });
        }

        tracing::info!(
            "Chain operation for chain {} stopped successfully",
            chain_id
        );
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
                // Create ERC20 contract instance
                let token_contract = ERC20::new(token_address, provider.clone());
                let transfer_amount = amount.into_evm_amount(6)?;

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
