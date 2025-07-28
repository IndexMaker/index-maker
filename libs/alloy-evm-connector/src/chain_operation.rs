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
                    tracing::info!("Chain operation {} cancelled", chain_id);
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
        tracing::info!("Executing command on chain {}", chain_id);

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
                tracing::info!(
                    "Executing ERC20 transfer: {} tokens from {:?} to {:?}",
                    amount,
                    from,
                    to
                );

                // Create ERC20 contract instance
                let token_contract = ERC20::new(token_address, provider.clone());

                let transfer_amount = U256::from(amount.to_u64().unwrap_or(0));

                match token_contract
                    .transferFrom(from, to, transfer_amount)
                    .send()
                    .await
                {
                    Ok(pending_tx) => {
                        tracing::info!(
                            "ERC20 transfer transaction sent, waiting for confirmation..."
                        );

                        match pending_tx.get_receipt().await {
                            Ok(receipt) => {
                                let tx_hash =
                                    format!("0x{}", hex::encode(receipt.transaction_hash));
                                tracing::info!("ERC20 transfer confirmed with hash: {}", tx_hash);

                                // Pass the original routing amounts from transfer_funds to the callback
                                tracing::info!(
                                    "Using original routing amounts - amount: {}, fee: {}",
                                    amount,
                                    cumulative_fee
                                );

                                // Call the callback with the original amounts from the routing system
                                if let Err(e) = callback(amount, cumulative_fee) {
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
                tracing::info!(
                    "Minting {} of {:?} for {:?} at price {} on {}",
                    quantity,
                    symbol,
                    recipient,
                    execution_price,
                    execution_time
                );

                // TODO: Implement actual index minting logic
                // For now, simulate successful minting

                tracing::info!("Index minting executed successfully on chain {}", chain_id);
                Ok(Some("0xmint_index...".to_string())) // Mock transaction hash
            }
            ChainCommand::BurnIndex {
                symbol,
                quantity,
                recipient,
                ..
            } => {
                tracing::info!("Burning {} of {:?} for {:?}", quantity, symbol, recipient);

                // TODO: Implement actual index burning logic
                // For now, simulate successful burning

                tracing::info!("Index burning executed successfully on chain {}", chain_id);
                Ok(Some("0xburn_index...".to_string())) // Mock transaction hash
            }
            ChainCommand::Withdraw {
                recipient,
                amount,
                execution_price,
                execution_time,
                ..
            } => {
                tracing::info!(
                    "Withdrawing {} to {:?} at price {} on {}",
                    amount,
                    recipient,
                    execution_price,
                    execution_time
                );

                // TODO: Implement actual withdrawal logic
                // For now, simulate successful withdrawal

                tracing::info!("Withdrawal executed successfully on chain {}", chain_id);
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
                    tracing::info!(
                        "Executing complete Across deposit flow: {} tokens from chain {} to chain {}",
                        deposit_amount, origin_chain_id, destination_chain_id
                    );

                    // Execute the complete Across deposit flow and get gas used
                    tracing::info!("Starting complete Across deposit execution...");
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
                            tracing::info!(
                                "Complete Across deposit flow executed successfully on chain {}, gas used: {}",
                                chain_id, gas_used
                            );

                            // Get current gas price from provider
                            let gas_price = provider.get_gas_price().await.map_err(|err| {
                                eyre::eyre!("Failed to obtain current price of gas: {:?}", err)
                            })?;

                            // Format gas price for display
                            let gas_price_formatted = format_units(U256::from(gas_price), "gwei")
                                .map_err(|err| {
                                eyre::eyre!("Failed to format gas price: {:?}", err)
                            })?;
                            tracing::info!("Current gas price: {} gwei", gas_price_formatted);

                            // Calculate gas fee in wei (gas_price * gas_used)
                            let fee_wei = U256::from(gas_price) * gas_used;

                            // Step 1: Format fee to 18 decimal places (ETH units) using alloy's format_units
                            let fee_eth_formatted_str =
                                format_units(fee_wei, "ether").map_err(|err| {
                                    eyre::eyre!("Failed to format gas fee to ETH units: {:?}", err)
                                })?;

                            // Step 2: Convert ETH to USDC using exchange rate
                            // Parse the formatted ETH string back to a decimal for calculation
                            let fee_eth_decimal = Decimal::from_str(&fee_eth_formatted_str)
                                .map_err(|err| {
                                    eyre::eyre!("Failed to parse ETH fee as decimal: {:?}", err)
                                })?;

                            // For now, use a simple ETH/USDC conversion rate (around $3000 per ETH)
                            // In production, this should fetch from a price oracle or API
                            let eth_to_usdc_rate = Decimal::from(3000);

                            // Convert ETH to USDC: fee_eth_decimal * eth_price = USDC value
                            let fee_usdc_decimal = fee_eth_decimal * eth_to_usdc_rate;

                            // Step 3: Parse USDC fee to 6 decimal units using parse_units
                            // Round to 6 decimal places for USDC precision first
                            let fee_usdc_rounded = fee_usdc_decimal.round_dp(6);
                            let fee_usdc_str = fee_usdc_rounded.to_string();

                            // Parse to 6-decimal units (converts to U256 with 6 decimal precision)
                            let fee_usdc_u256 = parse_units(&fee_usdc_str, 6).map_err(|err| {
                                eyre::eyre!("Failed to parse USDC fee to 6 decimals: {:?}", err)
                            })?;

                            // Convert fee from U256 to Amount (using the parsed 6-decimal value)
                            let fee_amount = Amount::try_from(fee_usdc_u256.to_string().as_str())
                                .map_err(|err| {
                                eyre::eyre!("Failed to convert USDC fee to Amount: {:?}", err)
                            })?;

                            tracing::info!(
                                "Gas fee calculation: {} ETH = {} USDC (at {} USD/ETH)",
                                fee_eth_formatted_str,
                                fee_usdc_rounded,
                                eth_to_usdc_rate
                            );

                            // Calculate updated deposit amount (subtract fee from original deposit amount)
                            let updated_deposit_amount = safe!(deposit_amount - fee_amount)
                                .ok_or_eyre("Failed to compute deposit amount")?;

                            // Calculate cumulative fee (add gas fee to existing cumulative fee)
                            let updated_cumulative_fee = safe!(cumulative_fee + fee_amount)
                                .ok_or_eyre("Failed to compute cumulative fee")?;

                            // Format amounts for display using format_units(value, 6)
                            let deposit_amount_u256 =
                                U256::from(updated_deposit_amount.to_u64().unwrap_or(0));
                            let formatted_deposit_amount = format_units(deposit_amount_u256, 6)
                                .map_err(|err| {
                                    eyre::eyre!("Failed to format deposit amount: {:?}", err)
                                })?;

                            let cumulative_fee_u256 =
                                U256::from(updated_cumulative_fee.to_u64().unwrap_or(0));
                            let formatted_cumulative_fee = format_units(cumulative_fee_u256, 6)
                                .map_err(|err| {
                                    eyre::eyre!("Failed to format cumulative fee: {:?}", err)
                                })?;

                            tracing::info!(
                                "Final amounts - deposit: {} USDC, cumulative fee: {} USDC",
                                formatted_deposit_amount,
                                formatted_cumulative_fee
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
