use alloy::{
    hex,
    primitives::{Address, FixedBytes, U256},
    providers::{Provider, ProviderBuilder},
    signers::local::LocalSigner,
};
use eyre::Result;
use rust_decimal::prelude::ToPrimitive;
use std::str::FromStr;
use symm_core::core::{async_loop::AsyncLoop, bits::Amount};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use tokio_util::sync::CancellationToken;

use crate::across_deposit::{create_verification_data, AcrossDepositBuilder};
use crate::commands::{ChainCommand, ChainOperationResult};
use crate::contracts::{AcrossConnector, OTCCustody, VerificationData, ERC20};
use crate::custody_helper::CAHelper;

/// Individual chain operation worker
/// Handles blockchain operations for a specific chain
pub struct ChainOperation {
    chain_id: u32,
    rpc_url: String,
    private_key: String,
    operation_loop: AsyncLoop<Result<()>>,
    result_sender: UnboundedSender<ChainOperationResult>,
}

impl ChainOperation {
    pub fn new(
        chain_id: u32,
        rpc_url: String,
        private_key: String,
        result_sender: UnboundedSender<ChainOperationResult>,
    ) -> Self {
        Self {
            chain_id,
            rpc_url,
            private_key,
            operation_loop: AsyncLoop::new(),
            result_sender,
        }
    }

    pub fn start(&mut self, mut command_receiver: UnboundedReceiver<ChainCommand>) -> Result<()> {
        let chain_id = self.chain_id;
        let rpc_url = self.rpc_url.clone();
        let private_key = self.private_key.clone();
        let result_sender = self.result_sender.clone();

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
                    println!(
                        "Failed to initialize AcrossDepositBuilder for chain {}: {}",
                        chain_id, e
                    );
                    None
                }
            };

        loop {
            tokio::select! {
                _ = cancel_token.cancelled() => {
                    println!("Chain operation {} cancelled", chain_id);
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
                        println!("Failed to send operation result: {}", e);
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
        println!("Executing command on chain {}", chain_id);

        match command {
            ChainCommand::SetSolverWeights { symbol, basket } => {
                // TODO: Implement solver weights setting
                println!("Setting solver weights for symbol: {:?}", symbol);
                Ok(None)
            }
            ChainCommand::SetupCustody {
                custody_id,
                input_token,
                amount,
                ..
            } => {
                if let Some(builder) = deposit_builder {
                    println!("Setting up custody {:?} with {} tokens", custody_id, amount);

                    // Execute setup custody
                    match builder
                        .setup_custody(
                            custody_id.into(),
                            alloy::primitives::Address::from_slice(input_token.as_slice()),
                            U256::from(amount.to_u64().unwrap_or(0)),
                        )
                        .await
                    {
                        Ok(()) => {
                            println!(
                                "Setup custody operation executed successfully on chain {}",
                                chain_id
                            );
                            Ok(Some("0xabc1...".to_string()))
                        }
                        Err(e) => Err(eyre::eyre!("Setup custody failed: {}", e)),
                    }
                } else {
                    Err(eyre::eyre!(
                        "AcrossDepositBuilder not initialized for chain {}",
                        chain_id
                    ))
                }
            }
            ChainCommand::ApproveToken {
                token_address,
                spender,
                amount,
                ..
            } => {
                if let Some(builder) = deposit_builder {
                    println!(
                        "Approving {:?} to spend {} tokens from {:?}",
                        spender, amount, token_address
                    );

                    // Execute token approval
                    match builder
                        .approve_input_token_for_custody(U256::from(amount.to_u64().unwrap_or(0)))
                        .await
                    {
                        Ok(()) => {
                            println!("Token approval executed successfully on chain {}", chain_id);
                            Ok(Some("0x2468...".to_string()))
                        }
                        Err(e) => Err(eyre::eyre!("Token approval failed: {}", e)),
                    }
                } else {
                    Err(eyre::eyre!(
                        "AcrossDepositBuilder not initialized for chain {}",
                        chain_id
                    ))
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
                // TODO: Implement index minting
                println!("Minting {} of {:?} for {:?}", quantity, symbol, recipient);
                Ok(Some("0x1234...".to_string())) // Mock transaction hash
            }
            ChainCommand::BurnIndex {
                symbol,
                quantity,
                recipient,
                ..
            } => {
                // TODO: Implement index burning
                println!("Burning {} of {:?} for {:?}", quantity, symbol, recipient);
                Ok(Some("0x5678...".to_string())) // Mock transaction hash
            }
            ChainCommand::Withdraw {
                recipient,
                amount,
                execution_price,
                execution_time,
                ..
            } => {
                // TODO: Implement withdrawal
                println!("Withdrawing {} for {:?}", amount, recipient);
                Ok(Some("0x9abc...".to_string())) // Mock transaction hash
            }
            ChainCommand::CustodyToConnector {
                input_token,
                amount,
                connector_address,
                custody_id,
                party,
                ..
            } => {
                if let Some(builder) = deposit_builder {
                    println!(
                        "Executing custodyToConnector: {} -> {:?}",
                        amount, connector_address
                    );

                    // Create CAHelper for this operation
                    let mut ca_helper =
                        CAHelper::new(chain_id as u64, crate::across_deposit::OTC_CUSTODY_ADDRESS);

                    // Add custodyToConnector action
                    let custody_action_index = ca_helper.custody_to_connector(
                        alloy::primitives::Address::from_slice(connector_address.as_slice()),
                        alloy::primitives::Address::from_slice(input_token.as_slice()),
                        0,
                        party.clone(),
                    );

                    // Generate merkle proof
                    let merkle_proof = ca_helper.get_merkle_proof(custody_action_index);

                    // Create verification data with current timestamp
                    let timestamp =
                        match crate::utils::get_current_timestamp(&builder.otc_custody.provider())
                            .await
                        {
                            Ok(t) => t + 3600,
                            Err(_) => 1234567890 + 3600, // Fallback timestamp
                        };
                    let verification_data = create_verification_data(
                        custody_id.into(),
                        0,
                        timestamp,
                        crate::contracts::CAKey {
                            parity: party.parity,
                            x: party.x,
                        },
                        crate::contracts::Signature {
                            e: FixedBytes([1u8; 32]),
                            s: FixedBytes([2u8; 32]),
                        },
                        merkle_proof,
                    );

                    // Execute custodyToConnector
                    match builder
                        .execute_custody_to_connector(
                            alloy::primitives::Address::from_slice(input_token.as_slice()),
                            U256::from(amount.to_u64().unwrap_or(0)),
                            verification_data,
                        )
                        .await
                    {
                        Ok(()) => {
                            println!(
                                "CustodyToConnector operation executed successfully on chain {}",
                                chain_id
                            );
                            Ok(Some("0xdef0...".to_string()))
                        }
                        Err(e) => Err(eyre::eyre!("CustodyToConnector failed: {}", e)),
                    }
                } else {
                    Err(eyre::eyre!(
                        "AcrossDepositBuilder not initialized for chain {}",
                        chain_id
                    ))
                }
            }
            ChainCommand::CallConnector {
                connector_address,
                calldata,
                custody_id,
                party,
                ..
            } => {
                if let Some(builder) = deposit_builder {
                    println!(
                        "Executing callConnector on {:?} with {} bytes of calldata",
                        connector_address,
                        calldata.len()
                    );

                    // Create CAHelper for this operation
                    let mut ca_helper =
                        CAHelper::new(chain_id as u64, crate::across_deposit::OTC_CUSTODY_ADDRESS);

                    // Add callConnector action
                    let connector_action_index = ca_helper.call_connector(
                        "AcrossConnector",
                        alloy::primitives::Address::from_slice(connector_address.as_slice()),
                        &calldata,
                        0u8,
                        party.clone(),
                    );

                    // Generate merkle proof
                    let merkle_proof = ca_helper.get_merkle_proof(connector_action_index);

                    // Create verification data with current timestamp
                    let timestamp =
                        match crate::utils::get_current_timestamp(&builder.otc_custody.provider())
                            .await
                        {
                            Ok(t) => t + 3600,
                            Err(_) => 1234567890 + 3600, // Fallback timestamp
                        };
                    let verification_data = create_verification_data(
                        custody_id.into(),
                        0,
                        timestamp,
                        crate::contracts::CAKey {
                            parity: party.parity,
                            x: party.x,
                        },
                        crate::contracts::Signature {
                            e: FixedBytes([1u8; 32]),
                            s: FixedBytes([2u8; 32]),
                        },
                        merkle_proof,
                    );

                    // Execute callConnector
                    match builder
                        .execute_call_connector(calldata, verification_data)
                        .await
                    {
                        Ok(receipt) => {
                            println!(
                                "CallConnector operation executed successfully on chain {}",
                                chain_id
                            );
                            Ok(Some(format!("0x{:x}", receipt.transaction_hash)))
                        }
                        Err(e) => Err(eyre::eyre!("CallConnector failed: {}", e)),
                    }
                } else {
                    Err(eyre::eyre!(
                        "AcrossDepositBuilder not initialized for chain {}",
                        chain_id
                    ))
                }
            }
            ChainCommand::GetAcrossSuggestedOutput {
                input_token,
                output_token,
                origin_chain_id,
                destination_chain_id,
                amount,
                ..
            } => {
                println!(
                    "Getting Across suggested output for {} from chain {} to chain {}",
                    amount, origin_chain_id, destination_chain_id
                );

                // Call Across API to get suggested output
                match crate::across_deposit::AcrossDepositBuilder::<P>::get_across_suggested_output(
                    alloy::primitives::Address::from_slice(input_token.as_slice()),
                    alloy::primitives::Address::from_slice(output_token.as_slice()),
                    origin_chain_id,
                    destination_chain_id,
                    alloy::primitives::U256::from(amount.to_u64().unwrap_or(0)),
                )
                .await
                {
                    Ok(suggested) => {
                        println!(
                            "Across suggested output retrieved successfully on chain {}",
                            chain_id
                        );
                        // Return structured result data
                        Ok(Some(format!(
                            "output:{},deadline:{},relayer:{:?},exclusivity:{}",
                            suggested.output_amount,
                            suggested.fill_deadline,
                            suggested.exclusive_relayer,
                            suggested.exclusivity_deadline
                        )))
                    }
                    Err(e) => Err(eyre::eyre!("GetAcrossSuggestedOutput failed: {}", e)),
                }
            }
            ChainCommand::EncodeDepositCalldata {
                recipient,
                input_token,
                output_token,
                deposit_amount,
                min_amount,
                destination_chain_id,
                exclusive_relayer,
                fill_deadline,
                exclusivity_deadline,
                ..
            } => {
                println!(
                    "Encoding deposit calldata for {} tokens to chain {}",
                    deposit_amount, destination_chain_id
                );

                // Create AcrossDeposit and encode calldata
                let deposit = crate::across_deposit::AcrossDeposit::new(
                    alloy::primitives::Address::from_slice(recipient.as_slice()),
                    alloy::primitives::Address::from_slice(input_token.as_slice()),
                    alloy::primitives::Address::from_slice(output_token.as_slice()),
                    alloy::primitives::U256::from(deposit_amount.to_u64().unwrap_or(0)),
                    alloy::primitives::U256::from(min_amount.to_u64().unwrap_or(0)),
                    destination_chain_id,
                    alloy::primitives::Address::from_slice(exclusive_relayer.as_slice()),
                    fill_deadline,
                    exclusivity_deadline,
                );

                let calldata = deposit.encode_deposit_calldata();
                println!(
                    "Deposit calldata encoded successfully on chain {} ({} bytes)",
                    chain_id,
                    calldata.len()
                );
                Ok(Some(format!("0x{}", hex::encode(&calldata))))
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
                    println!(
                        "Executing complete Across deposit flow: {} tokens from chain {} to chain {}",
                        deposit_amount, origin_chain_id, destination_chain_id
                    );

                    // Execute the complete Across deposit flow
                    println!("Starting complete Across deposit execution...");
                    match builder
                        .execute_complete_across_deposit(
                            alloy::primitives::Address::from_slice(recipient.as_slice()),
                            alloy::primitives::Address::from_slice(input_token.as_slice()),
                            alloy::primitives::Address::from_slice(output_token.as_slice()),
                            alloy::primitives::U256::from(deposit_amount.to_u64().unwrap_or(0)),
                            origin_chain_id,
                            destination_chain_id,
                        )
                        .await
                    {
                        Ok(()) => {
                            println!(
                                "✅ Complete Across deposit flow executed successfully on chain {}",
                                chain_id
                            );
                            let total_routed = Amount::ZERO;
                            let fee_deducted = Amount::ZERO;
                            callback(total_routed, fee_deducted).map_err(|err| {
                                eyre::eyre!(
                                    "ExecuteCompleteAcrossDeposit callback failed {:?}",
                                    err
                                )
                            })?;
                            todo!(
                                "Provide total amount routed and fee deducted in this single hop"
                            );
                            Ok(Some("0xacross_complete...".to_string()))
                        }
                        Err(e) => {
                            eprintln!("❌ ExecuteCompleteAcrossDeposit failed with error: {}", e);
                            eprintln!("Error details: {:?}", e);
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
            ChainCommand::GetCA { custody_id, .. } => {
                if let Some(builder) = deposit_builder {
                    println!("Getting CA information for custody ID: {:?}", custody_id);

                    // Get CA information
                    match builder.get_ca(custody_id.into()).await {
                        Ok(ca) => {
                            println!(
                                "CA information retrieved successfully on chain {}",
                                chain_id
                            );
                            Ok(Some(ca))
                        }
                        Err(e) => Err(eyre::eyre!("GetCA failed: {}", e)),
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
