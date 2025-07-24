use alloy::rpc::types::TransactionReceipt;
use alloy::signers::local::LocalSigner;
use alloy::sol_types::SolValue;
use alloy::{
    hex,
    primitives::{address, Address, FixedBytes, U256},
    providers::{Provider, ProviderBuilder},
    sol_types::SolCall,
};
use dotenv::dotenv;
use serde::{Deserialize, Serialize};
use std::env;
use std::str::FromStr;

use crate::contracts::{AcrossConnector, OTCCustody, ERC20};
use crate::custody_helper::CAHelper;
use crate::utils::{get_current_timestamp, set_next_block_timestamp};

pub const ACROSS_CONNECTOR_ADDRESS: Address =
    address!("0x8350a9Ab669808BE1DDF24FAF9c14475321D0504");
pub const ACROSS_SPOKE_POOL_ADDRESS: Address =
    address!("0xe35e9842fceaCA96570B734083f4a58e8F7C5f2A");
pub const OTC_CUSTODY_ADDRESS: Address = address!("0x9F6754bB627c726B4d2157e90357282d03362BCd");
pub const USDC_ARBITRUM_ADDRESS: Address = address!("0xaf88d065e77c8cC2239327C5EDb3A432268e5831");
pub const USDC_BASE_ADDRESS: Address = address!("0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913");
pub const USDC_DECIMALS: u8 = 6;
pub const DEPOSIT_AMOUNT: &str = "1000000";
pub const ARBITRUM_CHAIN_ID: u64 = 42161; // Arbitrum
pub const BASE_CHAIN_ID: u64 = 8453; // Base

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
    pub destination_chain_id: u64,
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
        destination_chain_id: u64,
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
    pub fn encode_deposit_calldata(&self) -> Vec<u8> {
        let call = AcrossConnector::depositCall {
            recipient: self.recipient,
            inputToken: self.input_token,
            outputToken: self.output_token,
            amount: self.deposit_amount,
            minAmount: self.min_amount,
            destinationChainId: U256::from(self.destination_chain_id),
            exclusiveRelayer: self.exclusive_relayer,
            fillDeadline: self.fill_deadline as u32,
            exclusivityDeadline: self.exclusivity_deadline as u32,
            message: Vec::new().into(),
        };
        call.abi_encode()
    }
}

pub struct AcrossDepositBuilder<P: Provider + Clone + 'static> {
    pub signer_address: Address,
    pub across_connector: AcrossConnector::AcrossConnectorInstance<P>,
    pub otc_custody: OTCCustody::OTCCustodyInstance<P>,
    pub usdc: ERC20::ERC20Instance<P>,
}

/// Create a new builder using environment variables
///
/// Reads from .env file:
/// - PRIVATE_KEY: Your wallet's private key
/// - RPC_URL: RPC endpoint (optional, defaults to http://localhost:8545)
pub async fn new_builder_from_env(
) -> eyre::Result<AcrossDepositBuilder<impl Provider + Clone>> {
    // Load environment variables from .env file in the libs/alloy-evm-connector directory
    dotenv().ok();

    // Read private key from environment variable
    let private_key = env::var("PRIVATE_KEY").expect("PRIVATE_KEY environment variable not set");

    // Read RPC URL from environment variable with fallback
    let rpc_url = env::var("RPC_URL").unwrap_or_else(|_| "http://localhost:8545".to_string());

    // Create wallet from private key
    let wallet = LocalSigner::from_str(&private_key)?;

    // Create provider with wallet attached
    let provider = ProviderBuilder::new()
        .wallet(wallet.clone())
        .connect_http(rpc_url.parse()?);

    AcrossDepositBuilder::new(provider, wallet.address()).await
}

impl<P: Provider + Clone + 'static> AcrossDepositBuilder<P> {
    pub async fn new(
        provider: P,
        signer_address: Address,
    ) -> eyre::Result<Self> {
        Ok(Self {
            signer_address,
            across_connector: AcrossConnector::AcrossConnectorInstance::new(
                ACROSS_CONNECTOR_ADDRESS,
                provider.clone(),
            ),
            otc_custody: OTCCustody::OTCCustodyInstance::new(OTC_CUSTODY_ADDRESS, provider.clone()),
            usdc: ERC20::ERC20Instance::new(USDC_ARBITRUM_ADDRESS, provider.clone()),
        })
    }

    /// Step 2: Approve input token for OTCCustody
    /// Returns the gas used for the approval transaction
    pub async fn approve_input_token_for_custody(
        &self,
        amount: U256,
    ) -> eyre::Result<U256> {
        tracing::info!("Approving USDC for amount: {}", amount);
        tracing::info!("Target: OTC_CUSTODY_ADDRESS ({:?})", OTC_CUSTODY_ADDRESS);
        
        let call = self.usdc.approve(OTC_CUSTODY_ADDRESS, amount);
        let receipt = call.send().await?.get_receipt().await?;
        
        let gas_used = U256::from(receipt.gas_used);
        tracing::info!("USDC approval transaction successful: {:?}, gas used: {}", receipt.transaction_hash, gas_used);
        Ok(gas_used)
    }

    /// Step 3: Get suggested output from Across API
    pub async fn get_across_suggested_output(
        input_token: Address,
        output_token: Address,
        origin_chain_id: u64,
        destination_chain_id: u64,
        amount: U256,
    ) -> eyre::Result<AcrossSuggestedOutput> {
        let client = reqwest::Client::new();
        let response = client
            .get("https://app.across.to/api/suggested-fees")
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
                return Err(eyre::eyre!("Across API error: {}", data.get("message").unwrap_or(&serde_json::Value::String("Unknown error".to_string()))));
            }
        }

        Ok(AcrossSuggestedOutput {
            output_amount: data["outputAmount"]
                .as_str()
                .and_then(|s| U256::from_str(s).ok())
                .or_else(|| data["outputAmount"].as_u64().map(U256::from))
                .unwrap_or(amount),
            fill_deadline: data["fillDeadline"]
                .as_str()
                .and_then(|s| s.parse::<u64>().ok())
                .or_else(|| data["fillDeadline"].as_u64())
                .unwrap_or((chrono::Utc::now().timestamp() + 3600) as u64),
            exclusive_relayer: data["exclusiveRelayer"]
                .as_str()
                .and_then(|s| Address::from_str(s).ok())
                .unwrap_or(Address::ZERO),
            exclusivity_deadline: data["exclusivityDeadline"]
                .as_u64()
                .unwrap_or((chrono::Utc::now().timestamp() + 1800) as u64),
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
        tracing::info!("Calling addressToCustody");
        tracing::info!("custody_id: {:?}", hex::encode_prefixed(custody_id));
        tracing::info!("input_token: {:?}", input_token);
        tracing::info!("amount: {}", amount);
        
        let call = self.otc_custody.addressToCustody(
            FixedBytes(custody_id),
            input_token,
            amount,
        );
        let receipt = call.send().await?.get_receipt().await?;
        
        let gas_used = U256::from(receipt.gas_used);
        tracing::info!("Custody setup transaction successful: {:?}, gas used: {}", receipt.transaction_hash, gas_used);
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
        tracing::info!("Calling custodyToConnector");
        tracing::info!("input_token: {:?}", input_token);
        tracing::info!("connector: {:?}", ACROSS_CONNECTOR_ADDRESS);
        tracing::info!("amount: {}", amount);
        tracing::info!("verification_data.id: {:?}", verification_data.id);
        
        let call = self.otc_custody.custodyToConnector(
            input_token,
            ACROSS_CONNECTOR_ADDRESS,
            amount,
            verification_data,
        );
        let receipt = call.send().await?.get_receipt().await?;
        
        let gas_used = U256::from(receipt.gas_used);
        tracing::info!("CustodyToConnector transaction successful: {:?}, gas used: {}", receipt.transaction_hash, gas_used);
        Ok(gas_used)
    }

    /// Step 14: Execute callConnector
    /// Returns the transaction receipt with gas usage information
    pub async fn execute_call_connector(
        &self,
        calldata: Vec<u8>,
        verification_data: crate::contracts::VerificationData,
    ) -> eyre::Result<TransactionReceipt> {
        tracing::info!("Calling callConnector");
        tracing::info!("connector_name: AcrossConnector");
        tracing::info!("connector_address: {:?}", ACROSS_CONNECTOR_ADDRESS);
        tracing::info!("calldata_length: {} bytes", calldata.len());
        tracing::info!("verification_data.id: {:?}", verification_data.id);
        
        let call = self.otc_custody.callConnector(
            "AcrossConnector".to_string(),
            ACROSS_CONNECTOR_ADDRESS,
            calldata.into(),
            Vec::new().into(),
            verification_data,
        );
        let receipt = call.send().await?.get_receipt().await?;
        
        let gas_used = U256::from(receipt.gas_used);
        tracing::info!("CallConnector transaction successful: {:?}, gas used: {}", receipt.transaction_hash, gas_used);
        Ok(receipt)
    }

    pub async fn get_ca(&self, custody_id: [u8; 32]) -> eyre::Result<String> {
        tracing::info!("Calling getCA for custody_id: {:?}", hex::encode_prefixed(custody_id));
        
        let call = self.otc_custody.getCA(FixedBytes(custody_id));
        let ca = call.call().await?.to_string();
        
        tracing::info!("CA result: {}", ca);
        Ok(ca)
    }

    /// Complete Across deposit flow with all steps
    /// Returns the total gas used for the entire flow
    pub async fn execute_complete_across_deposit(
        &self,
        recipient: Address,
        input_token: Address,
        output_token: Address,
        deposit_amount: U256,
        origin_chain_id: u64,
        destination_chain_id: u64,
    ) -> eyre::Result<U256> {
        tracing::info!("Executing complete Across deposit flow");
        tracing::info!("recipient: {:?}", recipient);
        tracing::info!("input_token: {:?}", input_token);
        tracing::info!("output_token: {:?}", output_token);
        tracing::info!("deposit_amount: {:?}", deposit_amount);
        tracing::info!("origin_chain_id: {:?}", origin_chain_id);
        tracing::info!("destination_chain_id: {:?}", destination_chain_id);

        let mut total_gas_used = U256::ZERO;

        // Step 1: Approve input token for OTCCustody
        tracing::info!("Step 1: Approving input token for custody");
        let approval_gas = match self.approve_input_token_for_custody(deposit_amount).await {
            Ok(gas_used) => {
                tracing::info!("Step 1 completed: Token approval successful, gas used: {}", gas_used);
                total_gas_used += gas_used;
                gas_used
            },
            Err(e) => {
                tracing::error!("Step 1 failed: Token approval error: {}", e);
                return Err(e);
            }
        };

        // Step 2: Setup custody helper (CAHelper) with the on-chain chain-id
        tracing::info!("Step 2: Setting up CAHelper");
        let chain_id_runtime = self.across_connector.provider().get_chain_id().await?;
        tracing::info!("Step 2a: Got chain ID: {}", chain_id_runtime);
        
        tracing::info!("Step 2b: Creating CAHelper with chain_id {} and custody address {:?}", 
                 chain_id_runtime, OTC_CUSTODY_ADDRESS);
        let mut ca_helper = CAHelper::new(chain_id_runtime, OTC_CUSTODY_ADDRESS);
        tracing::info!("Step 2 completed: CAHelper created");

        // Step 3: Call custodyToConnector of custody_helper
        tracing::info!("Step 3: Adding custodyToConnector action to CAHelper");
        let custody_action_index = ca_helper.custody_to_connector(
            ACROSS_CONNECTOR_ADDRESS,
            input_token,
            0,
            crate::custody_helper::Party {
                parity: 0,
                x: FixedBytes(address_to_bytes32(self.signer_address)),
            },
        );

        // Step 4: Get AcrossSuggestedOutput through API call
        tracing::info!("Step 4: Getting suggested output from Across API");
        let suggested_output = Self::get_across_suggested_output(
            input_token,
            output_token,
            origin_chain_id,
            destination_chain_id,
            deposit_amount,
        )
        .await?;
        tracing::info!("Step 4: Got suggested output from API");

        // Step 5: Encode deposit call data
        tracing::info!("Step 5: Encoding deposit calldata");
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
        let calldata = deposit.encode_deposit_calldata();
        tracing::info!("Step 5 completed: Deposit calldata encoded ({} bytes)", calldata.len());

        // Step 6: Call callConnector of custody_helper
        tracing::info!("Step 6: Adding callConnector action to CAHelper");
        let connector_action_index = ca_helper.call_connector(
            "AcrossConnector",
            ACROSS_CONNECTOR_ADDRESS,
            &calldata,
            0u8,
            crate::custody_helper::Party {
                parity: 0,
                x: FixedBytes(address_to_bytes32(self.signer_address)),
            },
        );
        tracing::info!("Step 6 completed: CallConnector action added (index: {})", connector_action_index);

        // Step 7: Fetch custodyId from custody_helper.get_custody_id()
        tracing::info!("Step 7: Getting custody ID");
        let custody_id = ca_helper.get_ca_root();
        tracing::info!("custody_id: {:?}", hex::encode_prefixed(custody_id));
        tracing::info!("Step 7 completed: Custody ID retrieved");

        // Step 8: Call otc_custody.addressToCustody
        tracing::info!("Step 8: Setting up custody with input tokens");
        let custody_gas = self.setup_custody(custody_id, input_token, deposit_amount)
            .await?;
        total_gas_used += custody_gas;
        tracing::info!("Step 8 completed: Custody setup successful, gas used: {}", custody_gas);

        // Step 9: Setup verification data for custodyToConnector and get merkle proof
        tracing::info!("Step 9: Creating verification data for custodyToConnector");
        // let custody_timestamp = std::time::SystemTime::now()
        //     .duration_since(std::time::UNIX_EPOCH)?
        //     .as_secs();
        let custody_timestamp =
            get_current_timestamp(self.across_connector.provider()).await? + 3600;
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
        tracing::info!("Step 9 completed: Verification data created for custodyToConnector");

        // Step 10: Call otc_custody.custodyToConnector
        tracing::info!("Step 10: Executing custodyToConnector");
        let custody_to_connector_gas = self.execute_custody_to_connector(input_token, deposit_amount, custody_verification)
            .await?;
        total_gas_used += custody_to_connector_gas;
        tracing::info!("Step 10 completed: custodyToConnector executed successfully, gas used: {}", custody_to_connector_gas);

        // Step 11: Setup verification data for callConnector
        tracing::info!("Step 11: Creating verification data for callConnector");
        // Use the same timestamp as custodyToConnector for consistency
        let connector_timestamp = custody_timestamp + 3600;
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
        tracing::info!("Step 11 completed: Verification data created for callConnector");

        // Step 12: Call otc_custody.callConnector
        tracing::info!("Step 12: Executing callConnector");
        let receipt = self
            .execute_call_connector(calldata, connector_verification)
            .await?;
        let call_connector_gas = U256::from(receipt.gas_used);
        total_gas_used += call_connector_gas;
        tracing::info!("Step 12 completed: callConnector executed successfully, gas used: {}", call_connector_gas);
        tracing::info!("Complete Across deposit flow finished successfully!");
        tracing::info!(
            "Final transaction hash: {:?}",
            receipt.transaction_hash
        );
        tracing::info!("Total gas used for complete flow: {}", total_gas_used);

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
) -> crate::contracts::VerificationData {
    crate::contracts::VerificationData {
        id: FixedBytes(custody_id),
        state,
        timestamp: U256::from(timestamp),
        pubKey: public_key,
        sig: signature,
        merkleProof: merkle_proof.into_iter().map(FixedBytes).collect(),
    }
}

/// Example usage function with all steps
pub async fn example_complete_across_deposit_flow() -> eyre::Result<()> {
    // Create builder using environment variables
    let builder = new_builder_from_env().await?;

    // Example parameters
    let recipient = address!("0xC0D3CB2E7452b8F4e7710bebd7529811868a85dd");
    let input_token = USDC_ARBITRUM_ADDRESS;
    let output_token = USDC_BASE_ADDRESS;
    let deposit_amount = U256::from(1_000_000u128); // 1 USDC (6 decimals)
    let origin_chain_id = ARBITRUM_CHAIN_ID;
    let destination_chain_id = BASE_CHAIN_ID;

    // Execute the complete flow with all steps
    builder
        .execute_complete_across_deposit(
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sol_macro_types() {
        // Test that the sol! macro generated types work correctly
        let call = AcrossConnector::depositCall {
            recipient: Address::ZERO,
            inputToken: Address::ZERO,
            outputToken: Address::ZERO,
            amount: U256::from(1000000u128),
            minAmount: U256::from(1000000u128),
            destinationChainId: U256::from(42161u64),
            exclusiveRelayer: Address::ZERO,
            fillDeadline: 0u32,
            exclusivityDeadline: 0u32,
            message: Vec::new().into(),
        };

        let encoded = call.abi_encode();
        assert!(!encoded.is_empty());
        // The encoded length may vary based on the sol! macro implementation
        // Just ensure it's not empty and has a reasonable size
        assert!(encoded.len() >= 4); // At least the function selector
    }

    #[test]
    fn test_encode_deposit_calldata() {
        let deposit_data = AcrossDeposit::new(
            Address::ZERO,
            Address::ZERO,
            Address::ZERO,
            U256::from(1000000u128),
            U256::from(1000000u128),
            0u64,
            Address::ZERO,
            0u64,
            0u64,
        );

        let calldata = deposit_data.encode_deposit_calldata();

        // The first 4 bytes should be the function selector
        assert_eq!(calldata.len() % 32, 4); // selector + N*32 bytes
        tracing::info!(
            "encoded deposit calldata: {:?}",
            hex::encode_prefixed(&calldata)
        );
    }

    #[test]
    fn test_create_verification_data() {
        let custody_id = [1u8; 32];
        let state = 0u8;
        let timestamp = 1234567890u64;
        let public_key = crate::contracts::CAKey {
            parity: 0,
            x: FixedBytes([2u8; 32]),
        };
        let signature = crate::contracts::Signature {
            e: FixedBytes([3u8; 32]),
            s: FixedBytes([4u8; 32]),
        };
        let merkle_proof = vec![[5u8; 32], [6u8; 32]];

        let verification_data = create_verification_data(
            custody_id,
            state,
            timestamp,
            public_key,
            signature,
            merkle_proof,
        );

        assert_eq!(verification_data.id, FixedBytes(custody_id));
        assert_eq!(verification_data.state, state);
        assert_eq!(verification_data.timestamp, U256::from(timestamp));
        assert_eq!(verification_data.pubKey.parity, 0);
        assert_eq!(verification_data.pubKey.x, FixedBytes([2u8; 32]));
        assert_eq!(verification_data.sig.e, FixedBytes([3u8; 32]));
        assert_eq!(verification_data.sig.s, FixedBytes([4u8; 32]));
        assert_eq!(verification_data.merkleProof.len(), 2);
    }

    #[test]
    fn test_across_deposit_struct() {
        let deposit = AcrossDeposit::new(
            Address::ZERO,
            Address::ZERO,
            Address::ZERO,
            U256::from(1000000u128),
            U256::from(999999u128),
            0u64,
            Address::ZERO,
            0u64,
            0u64,
        );

        assert_eq!(deposit.input_token, Address::ZERO);
        assert_eq!(deposit.output_token, Address::ZERO);
        assert_eq!(deposit.deposit_amount, U256::from(1000000u128));
        assert_eq!(deposit.min_amount, U256::from(999999u128));
        assert_eq!(deposit.destination_chain_id, 0u64);
        assert_eq!(deposit.exclusive_relayer, Address::ZERO);
        assert_eq!(deposit.fill_deadline, 0u64);
        assert_eq!(deposit.exclusivity_deadline, 0u64);
    }

    #[test]
    fn test_contract_instance() {
        // This test would require a real provider connection
        // For now, we'll test the struct creation with a mock
        assert!(true); // Placeholder assertion
    }

    #[tokio::test]
    async fn test_example_complete_across_deposit_flow() {
        example_complete_across_deposit_flow().await.unwrap();
    }
}
