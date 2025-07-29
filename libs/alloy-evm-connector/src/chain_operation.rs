use alloy::{
    hex,
    primitives::{
        utils::{format_units, parse_units},
        Address, FixedBytes, U256,
    },
    providers::{Provider, ProviderBuilder},
    signers::local::LocalSigner,
};
use chrono::Utc;
use eyre::OptionExt;
use eyre::Result;
use parking_lot::RwLock as AtomicLock;
use rust_decimal::prelude::ToPrimitive;
use rust_decimal::Decimal;
use safe_math::safe;
use std::str::FromStr;
use std::sync::Arc;
use symm_core::core::functional::{PublishSingle, SingleObserver};
use symm_core::core::{async_loop::AsyncLoop, bits::Amount, decimal_ext::DecimalExt};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use tokio_util::sync::CancellationToken;

use crate::across_deposit::{
    create_verification_data, AcrossDepositBuilder, USDC_ARBITRUM_ADDRESS, USDC_BASE_ADDRESS,
};
use crate::commands::{ChainCommand, ChainOperationResult};
use crate::contracts::{AcrossConnector, OTCCustody, VerificationData, ERC20};
use crate::custody_helper::CAHelper;
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

    pub fn start(&mut self, mut command_receiver: UnboundedReceiver<ChainCommand>) -> Result<()> {
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
        cancel_token: CancellationToken,
    ) -> Result<()> {
        // Initialize provider and contracts
        let wallet = LocalSigner::from_str(&private_key)?;
        let provider = ProviderBuilder::new()
            .wallet(wallet.clone())
            .connect_http(rpc_url.parse()?);

        // Initialize AcrossDepositBuilder for this chain
        let deposit_builder =
            match AcrossDepositBuilder::new(provider.clone(), wallet.address().clone()).await {
                Ok(builder) => Some(builder),
                Err(e) => {
                    tracing::error!(
                        "Failed to initialize AcrossDepositBuilder for chain {}: {}",
                        chain_id,
                        e
                    );
                    None
                }
            };

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
                let transfer_amount = U256::from(amount.to_u64().unwrap_or(0));

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

                                // Calculate gas fee
                                let gas_used = U256::from(receipt.gas_used);
                                let gas_price = provider.get_gas_price().await.map_err(|err| {
                                    eyre::eyre!("Failed to get gas price: {:?}", err)
                                })?;

                                let fee_wei = U256::from(gas_price) * gas_used;
                                let fee_eth_str =
                                    format_units(fee_wei, "ether").map_err(|err| {
                                        eyre::eyre!("Failed to format gas fee: {:?}", err)
                                    })?;

                                let fee_eth_decimal =
                                    Decimal::from_str(&fee_eth_str).map_err(|err| {
                                        eyre::eyre!("Failed to parse ETH fee: {:?}", err)
                                    })?;

                                // Convert ETH to USDC at $3000/ETH
                                let eth_to_usdc_rate = Decimal::from(3000);
                                let fee_usdc_decimal = fee_eth_decimal * eth_to_usdc_rate;
                                let fee_usdc_rounded = fee_usdc_decimal.round_dp(6);

                                let fee_usdc_u256 = parse_units(&fee_usdc_rounded.to_string(), 6)
                                    .map_err(|err| {
                                    eyre::eyre!("Failed to parse USDC fee: {:?}", err)
                                })?;

                                // Convert fee to proper Amount format (human-readable)
                                let fee_usdc_formatted = format_units(fee_usdc_u256, 6)
                                    .map_err(|err| eyre::eyre!("Failed to format fee: {:?}", err))?;
                                let fee_amount = Amount::try_from(fee_usdc_formatted.as_str())
                                    .map_err(|err| eyre::eyre!("Failed to convert fee to Amount: {:?}", err))?;

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
                            Err(e) => {
                                tracing::error!("Failed to get ERC20 transfer receipt: {}", e);
                                Err(eyre::eyre!("ERC20 transfer receipt failed: {}", e))
                            }
                        }
                    }
                    Err(e) => {
                        tracing::error!("ERC20 transfer transaction failed: {}", e);
                        Err(eyre::eyre!("ERC20 transfer failed: {}", e))
                    }
                }
            }
            ChainCommand::MintIndex {
                symbol,
                quantity,
                recipient,
                execution_price,
                execution_time,
                ..
            } => {
                // TODO: Implement actual index minting logic
                // For now, simulate successful minting
                Ok(Some("0xmint_index...".to_string())) // Mock transaction hash
            }
            ChainCommand::BurnIndex {
                symbol,
                quantity,
                recipient,
                ..
            } => {
                // TODO: Implement actual index burning logic
                // For now, simulate successful burning
                Ok(Some("0xburn_index...".to_string())) // Mock transaction hash
            }
            ChainCommand::Withdraw {
                recipient,
                amount,
                execution_price,
                execution_time,
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
                    // Use hardcoded USDC addresses: USDC_ARBITRUM_ADDRESS as input, USDC_BASE_ADDRESS as output
                    let input_token = USDC_ARBITRUM_ADDRESS;
                    let output_token = USDC_BASE_ADDRESS;

                    match builder
                        .execute_complete_across_deposit(
                            from,
                            to,
                            input_token,
                            output_token,
                            alloy::primitives::U256::from(
                                deposit_amount.to_u64().unwrap_or(1000000),
                            ),
                            origin_chain_id,
                            destination_chain_id,
                        )
                        .await
                    {
                        Ok(gas_used) => {
                            // Calculate gas fee
                            let gas_price = provider
                                .get_gas_price()
                                .await
                                .map_err(|err| eyre::eyre!("Failed to get gas price: {:?}", err))?;
                            let fee_wei = U256::from(gas_price) * gas_used;

                            let fee_eth_str = format_units(fee_wei, "ether").map_err(|err| {
                                eyre::eyre!("Failed to format gas fee: {:?}", err)
                            })?;

                            let fee_eth_decimal = Decimal::from_str(&fee_eth_str)
                                .map_err(|err| eyre::eyre!("Failed to parse ETH fee: {:?}", err))?;

                            // Convert ETH to USDC at $3000/ETH
                            let eth_to_usdc_rate = Decimal::from(3000);
                            let fee_usdc_decimal = fee_eth_decimal * eth_to_usdc_rate;
                            let fee_usdc_rounded = fee_usdc_decimal.round_dp(6);

                            let fee_usdc_u256 = parse_units(&fee_usdc_rounded.to_string(), 6)
                                .map_err(|err| {
                                    eyre::eyre!("Failed to parse USDC fee: {:?}", err)
                                })?;

                            // Convert fee to proper Amount format (human-readable)
                            let fee_usdc_formatted = format_units(fee_usdc_u256, 6)
                                .map_err(|err| eyre::eyre!("Failed to format fee: {:?}", err))?;
                            let fee_amount = Amount::try_from(fee_usdc_formatted.as_str())
                                .map_err(|err| eyre::eyre!("Failed to convert fee to Amount: {:?}", err))?;

                            // Calculate net amounts
                            let updated_deposit_amount = safe!(deposit_amount - fee_amount)
                                .ok_or_eyre("Failed to compute deposit amount")?;
                            let updated_cumulative_fee = safe!(cumulative_fee + fee_amount)
                                .ok_or_eyre("Failed to compute cumulative fee")?;

                            // Use Amount directly for display since it's already in human-readable format
                            tracing::info!("Across: {} USDC (fee: {}) tx: 0xacross_complete", 
                                          updated_deposit_amount, 
                                          updated_cumulative_fee);

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
                        Err(e) => {
                            tracing::error!(
                                "ExecuteCompleteAcrossDeposit failed with error: {}",
                                e
                            );
                            tracing::error!("Error details: {:?}", e);
                            Err(eyre::eyre!("ExecuteCompleteAcrossDeposit failed: {}", e))
                        }
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

    /// Publish chain notification events (for demo/testing purposes)
    fn publish_chain_events(
        chain_observer: &Arc<AtomicLock<SingleObserver<ChainNotification>>>,
        chain_id: u32,
    ) {
        // Simulate some chain events for demo purposes
        let observer = chain_observer.read();

        // Example: Deposit event
        observer.publish_single(ChainNotification::Deposit {
            chain_id,
            address: CoreAddress::new([0u8; 20]),
            amount: Amount::from(1000000), // 1 USDC
            timestamp: Utc::now(),
        });

        // Example: Withdrawal request event
        observer.publish_single(ChainNotification::WithdrawalRequest {
            chain_id,
            address: CoreAddress::new([0u8; 20]),
            amount: Amount::from(500000), // 0.5 USDC
            timestamp: Utc::now(),
        });

        // Example: Curator weights set event
        observer.publish_single(ChainNotification::CuratorWeightsSet(
            Symbol::from("BTC"),
            index_core::index::basket::BasketDefinition::try_new(Vec::new()).unwrap(),
        ));
    }
}
