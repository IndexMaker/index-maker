use alloy::{
    hex,
    primitives::{Address, FixedBytes, U256},
    providers::{Provider, ProviderBuilder},
    signers::local::LocalSigner,
};
use chrono::Utc;
use eyre::Result;
use parking_lot::RwLock as AtomicLock;
use rust_decimal::prelude::ToPrimitive;
use std::str::FromStr;
use std::sync::Arc;
use symm_core::core::{async_loop::AsyncLoop, bits::Amount};
use symm_core::core::functional::{PublishSingle, SingleObserver};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use tokio_util::sync::CancellationToken;

use crate::across_deposit::{create_verification_data, AcrossDepositBuilder};
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
                        chain_id, e
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
            ChainCommand::Erc20Transfer {
                from,
                to,
                amount,
                callback,
                ..
            } => {
                tracing::info!("Executing ERC20 transfer: {} tokens from {:?} to {:?}", amount, from, to);
                
                // TODO: Implement actual ERC20 transfer logic here
                // For now, simulate successful transfer
                let transferred_amount = amount;
                let fee = rust_decimal::dec!(0.0); // No fee for simple ERC20 transfer
                
                // Call the callback to publish the event
                if let Err(e) = callback(transferred_amount, fee) {
                    tracing::error!("Error in ERC20 transfer callback: {}", e);
                }
                
                tracing::info!("ERC20 transfer executed successfully on chain {}", chain_id);
                Ok(Some("0xerc20_transfer...".to_string())) // Mock transaction hash
            }
            ChainCommand::MintIndex {
                symbol,
                quantity,
                recipient,
                execution_price,
                execution_time,
                ..
            } => {
                tracing::info!("Minting {} of {:?} for {:?} at price {} on {}", 
                         quantity, symbol, recipient, execution_price, execution_time);
                
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
                tracing::info!("Withdrawing {} to {:?} at price {} on {}", 
                         amount, recipient, execution_price, execution_time);
                
                // TODO: Implement actual withdrawal logic
                // For now, simulate successful withdrawal
                
                tracing::info!("Withdrawal executed successfully on chain {}", chain_id);
                Ok(Some("0xwithdraw...".to_string())) // Mock transaction hash
            }
            ChainCommand::ExecuteCompleteAcrossDeposit {
                recipient,
                input_token,
                output_token,
                deposit_amount,
                origin_chain_id,
                destination_chain_id,
                party: _,
                callback,
                ..
            } => {
                if let Some(builder) = deposit_builder {
                    tracing::info!(
                        "Executing complete Across deposit flow: {} tokens from chain {} to chain {}",
                        deposit_amount, origin_chain_id, destination_chain_id
                    );

                    // Execute the complete Across deposit flow
                    tracing::info!("Starting complete Across deposit execution...");
                    match builder
                        .execute_complete_across_deposit(
                            alloy::primitives::Address::from_slice(&recipient.as_slice()[..20]),
                            alloy::primitives::Address::from_slice(&input_token.as_slice()[..20]),
                            alloy::primitives::Address::from_slice(&output_token.as_slice()[..20]),
                            alloy::primitives::U256::from(deposit_amount.to_u64().unwrap_or(0)),
                            origin_chain_id,
                            destination_chain_id,
                        )
                        .await
                    {
                        Ok(()) => {
                            tracing::info!(
                                "Complete Across deposit flow executed successfully on chain {}",
                                chain_id
                            );
                            
                            // For now, using the deposit amount as total routed and minimal fee
                            // In a real implementation, these would come from the actual transaction results
                            let total_routed = deposit_amount;
                            let fee_deducted = Amount::from_str("0.1").unwrap_or(Amount::ZERO); // Minimal fee for demo
                            
                            callback(total_routed, fee_deducted).map_err(|err| {
                                eyre::eyre!(
                                    "ExecuteCompleteAcrossDeposit callback failed {:?}",
                                    err
                                )
                            })?;
                            
                            Ok(Some("0xacross_complete...".to_string()))
                        }
                        Err(e) => {
                            tracing::error!("ExecuteCompleteAcrossDeposit failed with error: {}", e);
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
