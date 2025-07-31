use alloy::rpc::types::TransactionReceipt;
use alloy::signers::local::LocalSigner;
use alloy::sol_types::SolValue;
use alloy::{
    hex,
    primitives::{utils::format_units, Address, FixedBytes, U256},
    providers::{Provider, ProviderBuilder},
    sol_types::SolCall,
};
use rust_decimal::dec;
use serde::{Deserialize, Serialize};
use std::str::FromStr;

use crate::config::EvmConnectorConfig;
use crate::contracts::{AcrossConnector, OTCCustody, ERC20};
use crate::custody_helper::CAHelper;
use crate::utils::{get_current_timestamp, set_next_block_timestamp, IntoAmount, IntoEvmAmount};

pub const USDC_DECIMALS: u8 = 6;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AcrossSuggestedOutput {
    pub output_amount: U256,
    pub fill_deadline: u64,
    pub exclusive_relayer: Address,
    pub exclusivity_deadline: u64,
}

pub struct AcrossDeposit {
    pub recipient: Address,
    pub input_token: Address,
    pub output_token: Address,
    pub deposit_amount: U256,
    pub min_amount: U256,
    pub destination_chain_id: u32,
    pub exclusive_relayer: Address,
    pub fill_deadline: u64,
    pub exclusivity_deadline: u64,
    pub message: Vec<u8>,
}

impl AcrossDeposit {
    pub fn new(
        recipient: Address,
        input_token: Address,
        output_token: Address,
        deposit_amount: U256,
        min_amount: U256,
        destination_chain_id: u32,
        exclusive_relayer: Address,
        fill_deadline: u64,
        exclusivity_deadline: u64,
    ) -> Self {
        Self {
            recipient,
            input_token,
            output_token,
            deposit_amount,
            min_amount,
            destination_chain_id,
            exclusive_relayer,
            fill_deadline,
            exclusivity_deadline,
            message: Vec::new(),
        }
    }

    /// Task 2: Encode deposit calldata (standalone function)
    pub fn encode_deposit_calldata(&self) -> eyre::Result<Vec<u8>> {
        let call = AcrossConnector::depositCall {
            recipient: self.recipient,
            inputToken: self.input_token,
            outputToken: self.output_token,
            amount: self.deposit_amount,
            minAmount: self.min_amount,
            destinationChainId: self.destination_chain_id.into_evm_amount_default()?,
            exclusiveRelayer: self.exclusive_relayer,
            fillDeadline: self.fill_deadline as u32,
            exclusivityDeadline: self.exclusivity_deadline as u32,
            message: Vec::new().into(),
        };
        Ok(call.abi_encode())
    }
}

pub struct AcrossDepositBuilder<P: Provider + Clone + 'static> {
    pub signer_address: Address,
    pub across_connector: AcrossConnector::AcrossConnectorInstance<P>,
    pub otc_custody: OTCCustody::OTCCustodyInstance<P>,
    pub usdc: ERC20::ERC20Instance<P>,
}

/// Create a new builder using environment variables through config system
///
/// Uses centralized configuration from config.rs which reads from .env file:
/// - PRIVATE_KEY: Your wallet's private key
/// - DEFAULT_RPC_URL: RPC endpoint (with fallback)
pub async fn new_builder_from_env() -> eyre::Result<AcrossDepositBuilder<impl Provider + Clone>> {
    // Use config system for private key (following the new architecture)
    // Config system handles .env file loading and private key access
    let private_key = EvmConnectorConfig::get_private_key();

    // Use config system for RPC URL
    let rpc_url = EvmConnectorConfig::get_default_rpc_url();

    // Create wallet from private key
    let wallet = LocalSigner::from_str(&private_key)?;

    // Create provider with wallet attached
    let provider = ProviderBuilder::new()
        .wallet(wallet.clone())
        .connect_http(rpc_url.parse()?);

    AcrossDepositBuilder::new(provider, wallet.address()).await
}

impl<P: Provider + Clone + 'static> AcrossDepositBuilder<P> {
    pub async fn new(provider: P, signer_address: Address) -> eyre::Result<Self> {
        let config = EvmConnectorConfig::default();
        Ok(Self {
            signer_address,
            across_connector: AcrossConnector::AcrossConnectorInstance::new(
                config.bridge.across.connector_address,
                provider.clone(),
            ),
            otc_custody: OTCCustody::OTCCustodyInstance::new(
                config.bridge.across.custody_address,
                provider.clone(),
            ),
            usdc: ERC20::ERC20Instance::new(
                config.bridge.across.usdc_arbitrum_address,
                provider.clone(),
            ),
        })
    }

    /// Step 2: Approve input token for OTCCustody
    /// Returns the gas used for the approval transaction
    pub async fn approve_input_token_for_custody(&self, amount: U256) -> eyre::Result<U256> {
        let config = EvmConnectorConfig::default();
        let call = self
            .usdc
            .approve(config.bridge.across.custody_address, amount);
        let receipt = call.send().await?.get_receipt().await?;

        let gas_used = receipt.gas_used.into_evm_amount_default()?;
        Ok(gas_used)
    }

    /// Step 3: Get suggested output from Across API
    pub async fn get_across_suggested_output(
        input_token: Address,
        output_token: Address,
        origin_chain_id: u32,
        destination_chain_id: u32,
        amount: U256,
    ) -> eyre::Result<AcrossSuggestedOutput> {
        let client = reqwest::Client::new();
        let response = client
            .get(&EvmConnectorConfig::default().bridge.across.api_url)
            .query(&[
                ("inputToken", &input_token.to_string()),
                ("outputToken", &output_token.to_string()),
                ("originChainId", &origin_chain_id.to_string()),
                ("destinationChainId", &destination_chain_id.to_string()),
                ("amount", &amount.to_string()),
            ])
            .send()
            .await?;

        let data: serde_json::Value = response.json().await?;

        // Check if this is an error response
        if let Some(code) = data.get("code") {
            if code == "AMOUNT_TOO_LOW" {
                return Err(eyre::eyre!(
                    "Across API error: {}",
                    data.get("message")
                        .unwrap_or(&serde_json::Value::String("Unknown error".to_string()))
                ));
            }
        }

        Ok(AcrossSuggestedOutput {
            output_amount: data["outputAmount"]
                .as_str()
                .and_then(|s| U256::from_str(s).ok())
                .or_else(|| {
                    data["outputAmount"]
                        .as_u64()
                        .and_then(|val| val.into_evm_amount_usdc().ok())
                })
                .unwrap_or(amount),
            fill_deadline: data["fillDeadline"]
                .as_str()
                .and_then(|s| s.parse::<u64>().ok())
                .or_else(|| data["fillDeadline"].as_u64())
                .unwrap_or(
                    (chrono::Utc::now().timestamp()
                        + EvmConnectorConfig::get_filldeadline_buffer() as i64)
                        as u64,
                ),
            exclusive_relayer: data["exclusiveRelayer"]
                .as_str()
                .and_then(|s| Address::from_str(s).ok())
                .unwrap_or(Address::ZERO),
            exclusivity_deadline: data["exclusivityDeadline"].as_u64().unwrap_or(
                (chrono::Utc::now().timestamp()
                    + EvmConnectorConfig::get_exclusivity_deadline_buffer() as i64)
                    as u64,
            ),
        })
    }

    /// Step 10: Setup custody with input tokens
    /// Returns the gas used for the custody setup transaction
    pub async fn setup_custody(
        &self,
        custody_id: [u8; 32],
        input_token: Address,
        amount: U256,
    ) -> eyre::Result<U256> {
        let call = self
            .otc_custody
            .addressToCustody(FixedBytes(custody_id), input_token, amount);
        let receipt = call.send().await?.get_receipt().await?;

        let gas_used = receipt.gas_used.into_evm_amount_default()?;
        Ok(gas_used)
    }

    /// Step 12: Execute custodyToConnector
    /// Returns the gas used for the custodyToConnector transaction
    pub async fn execute_custody_to_connector(
        &self,
        input_token: Address,
        amount: U256,
        verification_data: crate::contracts::VerificationData,
    ) -> eyre::Result<U256> {
        let config = EvmConnectorConfig::default();
        let call = self.otc_custody.custodyToConnector(
            input_token,
            config.bridge.across.connector_address,
            amount,
            verification_data,
        );
        let receipt = call.send().await?.get_receipt().await?;

        let gas_used = receipt.gas_used.into_evm_amount_default()?;
        Ok(gas_used)
    }

    /// Step 14: Execute callConnector
    /// Returns the transaction receipt with gas usage information
    pub async fn execute_call_connector(
        &self,
        calldata: Vec<u8>,
        verification_data: crate::contracts::VerificationData,
    ) -> eyre::Result<TransactionReceipt> {
        let config = EvmConnectorConfig::default();
        let call = self.otc_custody.callConnector(
            "AcrossConnector".to_string(),
            config.bridge.across.connector_address,
            calldata.into(),
            Vec::new().into(),
            verification_data,
        );
        let receipt = call.send().await?.get_receipt().await?;

        Ok(receipt)
    }

    pub async fn get_ca(&self, custody_id: [u8; 32]) -> eyre::Result<String> {
        tracing::info!(
            "Calling getCA for custody_id: {:?}",
            hex::encode_prefixed(custody_id)
        );

        let call = self.otc_custody.getCA(FixedBytes(custody_id));
        let ca = call.call().await?.to_string();

        tracing::info!("CA result: {}", ca);
        Ok(ca)
    }

    /// Complete Across deposit flow with all steps
    /// Returns the total gas used for the entire flow
    pub async fn execute_complete_across_deposit(
        &self,
        _sender: Address,
        recipient: Address,
        input_token: Address,
        output_token: Address,
        deposit_amount: U256,
        origin_chain_id: u32,
        destination_chain_id: u32,
    ) -> eyre::Result<U256> {
        let mut total_gas_used = U256::ZERO;

        // Log the start of the deposit
        tracing::info!(
            "Starting Across deposit: {} USDC {} -> {}",
            deposit_amount.into_amount_usdc().unwrap(),
            if origin_chain_id
                == EvmConnectorConfig::default()
                    .get_chain_id("arbitrum")
                    .unwrap()
            {
                "ARBITRUM"
            } else {
                "UNKNOWN"
            },
            if destination_chain_id == EvmConnectorConfig::default().get_chain_id("base").unwrap() {
                "BASE"
            } else {
                "UNKNOWN"
            }
        );

        // Approve input token for OTCCustody
        let approval_gas = self.approve_input_token_for_custody(deposit_amount).await?;
        total_gas_used += approval_gas;

        // Setup custody helper
        let chain_id_runtime = self.across_connector.provider().get_chain_id().await? as u32;
        let config = EvmConnectorConfig::default();
        let mut ca_helper = CAHelper::new(chain_id_runtime, config.bridge.across.custody_address);

        // Step 3: Call custodyToConnector of custody_helper
        let custody_action_index = ca_helper.custody_to_connector(
            config.bridge.across.connector_address,
            input_token,
            0,
            crate::custody_helper::Party {
                parity: 0,
                x: FixedBytes(address_to_bytes32(self.signer_address)),
            },
        );

        // Step 4: Get AcrossSuggestedOutput through API call
        let suggested_output = Self::get_across_suggested_output(
            input_token,
            output_token,
            origin_chain_id,
            destination_chain_id,
            deposit_amount,
        )
        .await?;

        // Step 5: Encode deposit call data
        let deposit = AcrossDeposit::new(
            recipient,
            input_token,
            output_token,
            deposit_amount,
            suggested_output.output_amount,
            destination_chain_id,
            suggested_output.exclusive_relayer,
            suggested_output.fill_deadline,
            suggested_output.exclusivity_deadline,
        );
        let calldata = deposit.encode_deposit_calldata()?;

        // Step 6: Call callConnector of custody_helper
        let connector_action_index = ca_helper.call_connector(
            "AcrossConnector",
            config.bridge.across.connector_address,
            &calldata,
            0u8,
            crate::custody_helper::Party {
                parity: 0,
                x: FixedBytes(address_to_bytes32(self.signer_address)),
            },
        );

        // Step 7: Fetch custodyId from custody_helper.get_custody_id()
        let custody_id = ca_helper.get_ca_root();

        // Step 8: Call otc_custody.addressToCustody
        tracing::info!("Setting up custody and transferring funds to bridge...");
        let custody_gas = self
            .setup_custody(custody_id, input_token, deposit_amount)
            .await?;
        total_gas_used += custody_gas;

        // Step 9: Setup verification data for custodyToConnector and get merkle proof
        // let custody_timestamp = std::time::SystemTime::now()
        //     .duration_since(std::time::UNIX_EPOCH)?
        //     .as_secs();
        let custody_timestamp = get_current_timestamp(self.across_connector.provider()).await?
            + EvmConnectorConfig::get_filldeadline_buffer();
        set_next_block_timestamp(self.across_connector.provider(), custody_timestamp).await?;
        let custody_verification = create_verification_data(
            custody_id,
            0,
            custody_timestamp,
            crate::contracts::CAKey {
                parity: 0,
                x: FixedBytes(address_to_bytes32(self.signer_address)),
            },
            crate::contracts::Signature {
                e: FixedBytes(rand::random::<[u8; 32]>()),
                s: FixedBytes(rand::random::<[u8; 32]>()),
            },
            ca_helper.get_merkle_proof(custody_action_index),
        );

        // Step 10: Call otc_custody.custodyToConnector
        let custody_to_connector_gas = self
            .execute_custody_to_connector(input_token, deposit_amount, custody_verification?)
            .await?;
        total_gas_used += custody_to_connector_gas;

        // Step 11: Setup verification data for callConnector
        // Use the same timestamp as custodyToConnector for consistency
        let connector_timestamp = custody_timestamp + EvmConnectorConfig::get_filldeadline_buffer();
        set_next_block_timestamp(self.across_connector.provider(), connector_timestamp).await?;
        let connector_proof = ca_helper.get_merkle_proof(connector_action_index);
        let connector_verification = create_verification_data(
            custody_id,
            0,
            connector_timestamp,
            crate::contracts::CAKey {
                parity: 0,
                x: FixedBytes(address_to_bytes32(self.signer_address)),
            },
            crate::contracts::Signature {
                e: FixedBytes(rand::random::<[u8; 32]>()),
                s: FixedBytes(rand::random::<[u8; 32]>()),
            },
            connector_proof,
        );

        // Step 12: Call otc_custody.callConnector
        let receipt = self
            .execute_call_connector(calldata, connector_verification?)
            .await?;
        let call_connector_gas = receipt.gas_used.into_evm_amount_default()?;
        total_gas_used += call_connector_gas;

        // Log completion with final transaction hash and gas usage
        tracing::info!(
            "Across deposit completed - tx: {:?}, total gas: {}",
            receipt.transaction_hash,
            total_gas_used
        );

        Ok(total_gas_used)
    }
}

/// Task 6: Create verification data (standalone function)
pub fn create_verification_data(
    custody_id: [u8; 32],
    state: u8,
    timestamp: u64,
    public_key: crate::contracts::CAKey,
    signature: crate::contracts::Signature,
    merkle_proof: Vec<[u8; 32]>,
) -> eyre::Result<crate::contracts::VerificationData> {
    Ok(crate::contracts::VerificationData {
        id: FixedBytes(custody_id),
        state,
        timestamp: timestamp.into_evm_amount_default()?,
        pubKey: public_key,
        sig: signature,
        merkleProof: merkle_proof.into_iter().map(FixedBytes).collect(),
    })
}

/// Example usage function with all steps
pub async fn example_complete_across_deposit_flow() -> eyre::Result<()> {
    // Create builder using environment variables
    let builder = new_builder_from_env().await?;

    // Example parameters
    let config = EvmConnectorConfig::default();
    let sender = EvmConnectorConfig::get_default_sender_address();
    let recipient = EvmConnectorConfig::get_default_sender_address();
    let input_token = config.get_usdc_address("arbitrum").unwrap();
    let output_token = config.get_usdc_address("base").unwrap();
    let deposit_amount = dec!(10.0).into_evm_amount_usdc()?;
    let origin_chain_id = config.get_chain_id("arbitrum").unwrap();
    let destination_chain_id = config.get_chain_id("base").unwrap();

    // Execute the complete flow with all steps
    builder
        .execute_complete_across_deposit(
            sender,
            recipient,
            input_token,
            output_token,
            deposit_amount,
            origin_chain_id,
            destination_chain_id,
        )
        .await?;

    tracing::info!("Complete Across deposit flow executed successfully!");
    Ok(())
}

/// Left-pad a 20-byte `Address` into a 32-byte array (Solidity bytes32(bytes20(addr))).
fn address_to_bytes32(addr: Address) -> [u8; 32] {
    let mut out = [0u8; 32];
    // copy the 20-byte address into the lower 20 bytes (right-aligned)
    out[12..].copy_from_slice(addr.as_slice());
    out
}
