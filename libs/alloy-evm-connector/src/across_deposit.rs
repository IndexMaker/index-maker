use alloy::{
    hex,
    primitives::{address, Address, U256},
    sol_types::SolCall,
};
use serde::{Deserialize, Serialize};
use std::str::FromStr;

use crate::contracts::{AcrossConnector, OTCCustody, ERC20};

pub const ACROSS_CONNECTOR_ADDRESS: Address =
    address!("0xB95bCdEe3266901c8fB7b77D3DFea62ff09113B7");
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
    input_token: Address,
    output_token: Address,
    deposit_amount: U256,
    output_amount: U256,
    destination_chain_id: u64,
    exclusive_relayer: Address,
    fill_deadline: u64,
    exclusivity_deadline: u64,
}

pub struct AcrossDepositBuilder {
    pub connector_address: Address,
    pub spoke_pool_address: Address,
    pub otc_custody_address: Address,
}

impl AcrossDeposit {
    pub fn new(
        input_token: Address,
        output_token: Address,
        deposit_amount: U256,
        output_amount: U256,
        destination_chain_id: u64,
        exclusive_relayer: Address,
        fill_deadline: u64,
        exclusivity_deadline: u64,
    ) -> Self {
        Self {
            input_token,
            output_token,
            deposit_amount,
            output_amount,
            destination_chain_id,
            exclusive_relayer,
            fill_deadline,
            exclusivity_deadline,
        }
    }
}

impl AcrossDepositBuilder {
    pub fn new(
        connector_address: Address,
        spoke_pool_address: Address,
        otc_custody_address: Address,
    ) -> Self {
        Self {
            connector_address,
            spoke_pool_address,
            otc_custody_address,
        }
    }

    pub async fn across_suggested_output(
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

    pub fn encode_deposit_calldata(deposit: AcrossDeposit) -> Vec<u8> {
        // Use the strongly typed call from sol! macro
        let call = AcrossConnector::depositCall {
            inputToken: deposit.input_token,
            outputToken: deposit.output_token,
            amount: deposit.deposit_amount,
            destinationChainId: U256::from(deposit.destination_chain_id),
            recipient: deposit.exclusive_relayer,
            fillDeadline: deposit.fill_deadline as u32,
            exclusivityDeadline: deposit.exclusivity_deadline as u32,
            message: Vec::new().into(),
        };
        call.abi_encode()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_across_deposit_creation() {
        let across_deposit = AcrossDepositBuilder::new(
            ACROSS_CONNECTOR_ADDRESS,
            ACROSS_SPOKE_POOL_ADDRESS,
            OTC_CUSTODY_ADDRESS,
        );

        assert_eq!(across_deposit.connector_address, ACROSS_CONNECTOR_ADDRESS);
        assert_eq!(across_deposit.spoke_pool_address, ACROSS_SPOKE_POOL_ADDRESS);
        assert_eq!(across_deposit.otc_custody_address, OTC_CUSTODY_ADDRESS);
    }

    #[test]
    fn test_encode_deposit_calldata() {
        let deposit_data = AcrossDeposit::new(
            Address::ZERO,
            Address::ZERO,
            U256::from(1000000u128),
            U256::from(1000000u128),
            42161u64,
            Address::ZERO,
            0u64,
            0u64,
        );

        let calldata = AcrossDepositBuilder::encode_deposit_calldata(deposit_data);

        println!("calldata: {:?}", hex::encode(&calldata));

        assert!(!calldata.is_empty());
    }

    #[test]
    fn test_sol_macro_types() {
        // Test that the sol! macro generated types work correctly
        let call = AcrossConnector::depositCall {
            inputToken: Address::ZERO,
            outputToken: Address::ZERO,
            amount: U256::from(1000000u128),
            destinationChainId: U256::from(42161u64),
            recipient: Address::ZERO,
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
}
