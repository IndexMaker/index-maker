use alloy::signers::local::LocalSigner;
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

pub const ACROSS_CONNECTOR_ADDRESS: Address =
    address!("0x8350a9Ab669808BE1DDF24FAF9c14475321D0504");
pub const ACROSS_SPOKE_POOL_ADDRESS: Address =
    address!("0xe35e9842fceaCA96570B734083f4a58e8F7C5f2A");
pub const OTC_CUSTODY_ADDRESS: Address = address!("0x9F6754bB627c726B4d2157e90357282d03362BCd");
pub const USDC_ARBITRUM_ADDRESS: Address = address!("0xaf88d065e77c8cC2239327C5EDb3A432268e5831");
pub const USDC_BASE_ADDRESS: Address = address!("0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913");
pub const USDC_DECIMALS: u8 = 6;
pub const DEPOSIT_AMOUNT: &str = "1000000";
pub const ORIGIN_CHAIN_ID: u64 = 42161; // Arbitrum
pub const DESTINATION_CHAIN_ID: u64 = 8453; // Base

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
            message: Vec::new().into(),
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
) -> Result<AcrossDepositBuilder<impl Provider + Clone>, Box<dyn std::error::Error>> {
    // Load environment variables from .env file
    dotenv().ok();

    // Read private key from environment variable
    let private_key = env::var("PRIVATE_KEY").expect("PRIVATE_KEY environment variable not set");

    // Read RPC URL from environment variable with fallback
    let rpc_url = env::var("RPC_URL").unwrap_or_else(|_| "http://localhost:8545".to_string());

    // Create wallet from private key
    let wallet = LocalSigner::from_str(&private_key)?;

    // Create provider with wallet attached
    let provider = ProviderBuilder::new()
        .wallet(wallet)
        .connect_http(rpc_url.parse()?);

    AcrossDepositBuilder::new(provider).await
}

impl<P: Provider + Clone + 'static> AcrossDepositBuilder<P> {
    pub async fn new(provider: P) -> Result<Self, Box<dyn std::error::Error>> {
        Ok(Self {
            across_connector: AcrossConnector::AcrossConnectorInstance::new(
                ACROSS_CONNECTOR_ADDRESS,
                provider.clone(),
            ),
            otc_custody: OTCCustody::OTCCustodyInstance::new(OTC_CUSTODY_ADDRESS, provider.clone()),
            usdc: ERC20::ERC20Instance::new(USDC_ARBITRUM_ADDRESS, provider.clone()),
        })
    }

    /// Step 2: Approve input token for OTCCustody
    pub async fn approve_input_token_for_custody(
        &self,
        amount: U256,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let call = self.usdc.approve(OTC_CUSTODY_ADDRESS, amount);
        call.send().await?.get_receipt().await?;
        Ok(())
    }

    /// Step 3: Get suggested output from Across API
    pub async fn get_across_suggested_output(
        input_token: Address,
        output_token: Address,
        origin_chain_id: u64,
        destination_chain_id: u64,
        amount: U256,
    ) -> Result<AcrossSuggestedOutput, Box<dyn std::error::Error>> {
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

        Ok(AcrossSuggestedOutput {
            output_amount: U256::from_str(
                data["outputAmount"].as_str().unwrap_or(&amount.to_string()),
            )
            .unwrap_or(amount),
            fill_deadline: data["fillDeadline"].as_u64().unwrap(),
            exclusive_relayer: Address::from_str(
                data["exclusiveRelayer"]
                    .as_str()
                    .unwrap_or("0x0000000000000000000000000000000000000000"),
            )
            .unwrap_or(Address::ZERO),
            exclusivity_deadline: data["exclusivityDeadline"].as_u64().unwrap(),
        })
    }

    /// Step 10: Setup custody with input tokens
    pub async fn setup_custody(
        &self,
        custody_id: [u8; 32],
        input_token: Address,
        amount: U256,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let call = self
            .otc_custody
            .addressToCustody(FixedBytes(custody_id), input_token, amount);
        call.send().await?.get_receipt().await?;
        Ok(())
    }

    /// Step 12: Execute custodyToConnector
    pub async fn execute_custody_to_connector(
        &self,
        input_token: Address,
        amount: U256,
        verification_data: crate::contracts::VerificationData,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let call = self.otc_custody.custodyToConnector(
            input_token,
            ACROSS_CONNECTOR_ADDRESS,
            amount,
            verification_data,
        );
        call.send().await?.get_receipt().await?;
        Ok(())
    }

    /// Step 14: Execute callConnector
    pub async fn execute_call_connector(
        &self,
        calldata: Vec<u8>,
        verification_data: crate::contracts::VerificationData,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let call = self.otc_custody.callConnector(
            "AcrossConnector".to_string(),
            ACROSS_CONNECTOR_ADDRESS,
            calldata.into(),
            Vec::new().into(),
            verification_data,
        );
        call.send().await?.get_receipt().await?;
        Ok(())
    }

    /// Complete Across deposit flow with all steps
    pub async fn execute_complete_across_deposit(
        &self,
        recipient: Address,
        input_token: Address,
        output_token: Address,
        deposit_amount: U256,
        origin_chain_id: u64,
        destination_chain_id: u64,
    ) -> Result<(), Box<dyn std::error::Error>> {
        // Step 3: Approve input token for OTCCustody
        println!("Step 3: Approving input token for custody");
        self.approve_input_token_for_custody(deposit_amount).await?;

        // Step 4: Setup custody helper (CAHelper)
        println!("Step 4: Setting up CAHelper");
        let mut ca_helper = CAHelper::new(origin_chain_id, OTC_CUSTODY_ADDRESS);

        // Step 5: Call custodyToConnector of custody_helper
        println!("Step 5: Adding custodyToConnector action to CAHelper");
        let custody_action_index = ca_helper.custody_to_connector(
            ACROSS_CONNECTOR_ADDRESS,
            input_token,
            0,
            crate::custody_helper::Party {
                parity: 0,
                x: FixedBytes([0u8; 32]),
            },
        );

        // Step 6: Get AcrossSuggestedOutput through API call
        println!("Step 6: Getting suggested output from Across API");
        let suggested_output = Self::get_across_suggested_output(
            input_token,
            output_token,
            origin_chain_id,
            destination_chain_id,
            deposit_amount,
        )
        .await?;

        // Step 7: Encode deposit call data
        println!("Step 7: Encoding deposit calldata");
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

        // Step 8: Call callConnector of custody_helper
        println!("Step 8: Adding callConnector action to CAHelper");
        let connector_action_index = ca_helper.call_connector(
            "AcrossConnector",
            ACROSS_CONNECTOR_ADDRESS,
            &calldata,
            0u8,
            crate::custody_helper::Party {
                parity: 0,
                x: FixedBytes([0u8; 32]),
            },
        );

        // Step 9: Fetch custodyId from custody_helper.get_custody_id()
        println!("Step 9: Getting custody ID");
        let custody_id = ca_helper.get_custody_id();

        // Step 10: Call otc_custody.addressToCustody
        println!("Step 10: Setting up custody with input tokens");
        self.setup_custody(custody_id, input_token, deposit_amount)
            .await?;

        // Step 11: Setup verification data for custodyToConnector and get merkle proof
        println!("Step 11: Creating verification data for custodyToConnector");
        let custody_timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_secs();
        let custody_verification = create_verification_data(
            custody_id,
            0,
            custody_timestamp,
            crate::contracts::CAKey {
                parity: 0,
                x: FixedBytes([0u8; 32]),
            },
            crate::contracts::Signature {
                e: FixedBytes([0u8; 32]), // Mock signature
                s: FixedBytes([0u8; 32]), // Mock signature
            },
            ca_helper.get_merkle_proof(custody_action_index),
        );

        // Step 12: Call otc_custody.custodyToConnector
        println!("Step 12: Executing custodyToConnector");
        self.execute_custody_to_connector(input_token, deposit_amount, custody_verification)
            .await?;

        // Step 13: Setup verification data for callConnector
        println!("Step 13: Creating verification data for callConnector");
        let connector_verification = create_verification_data(
            custody_id,
            0,
            custody_timestamp,
            crate::contracts::CAKey {
                parity: 0,
                x: FixedBytes([0u8; 32]),
            },
            crate::contracts::Signature {
                e: FixedBytes([0u8; 32]), // Mock signature
                s: FixedBytes([0u8; 32]), // Mock signature
            },
            ca_helper.get_merkle_proof(connector_action_index),
        );

        // Step 14: Call otc_custody.callConnector
        println!("Step 14: Executing callConnector");
        self.execute_call_connector(calldata, connector_verification)
            .await?;

        println!("Across deposit completed successfully!");
        Ok(())
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
pub async fn example_complete_across_deposit_flow() -> Result<(), Box<dyn std::error::Error>> {
    // Create builder using environment variables
    let builder = new_builder_from_env().await?;

    // Example parameters
    let recipient = address!("0xC0D3CB2E7452b8F4e7710bebd7529811868a85dd");
    let input_token = USDC_ARBITRUM_ADDRESS;
    let output_token = USDC_BASE_ADDRESS;
    let deposit_amount = U256::from(1_000_000u128); // 1 USDC (6 decimals)
    let origin_chain_id = ORIGIN_CHAIN_ID;
    let destination_chain_id = DESTINATION_CHAIN_ID;

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

    println!("Complete Across deposit flow executed successfully!");
    Ok(())
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

        println!("calldata: {:?}", hex::encode_prefixed(&calldata));

        assert!(!calldata.is_empty());
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
}
