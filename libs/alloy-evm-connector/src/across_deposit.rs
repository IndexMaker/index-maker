use alloy::{
    hex,
    json_abi::JsonAbi,
    primitives::{Address, B256, U256},
    sol_types::SolValue,
};
use serde::{Deserialize, Serialize};
use std::str::FromStr;

use crate::abis::{ACROSS_CONNECTOR_ABI, ERC20_ABI, OTC_CUSTODY_ABI};

pub const ACROSS_CONNECTOR_ADDRESS: &str = "0xB95bCdEe3266901c8fB7b77D3DFea62ff09113B7";
pub const ACROSS_SPOKE_POOL_ADDRESS: &str = "0xe35e9842fceaCA96570B734083f4a58e8F7C5f2A";
pub const OTC_CUSTODY_ADDRESS: &str = "0x9F6754bB627c726B4d2157e90357282d03362BCd";
pub const USDC_ARBITRUM_ADDRESS: &str = "0xaf88d065e77c8cC2239327C5EDb3A432268e5831";
pub const USDC_BASE_ADDRESS: &str = "0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913";
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
    pub otc_custody_abi: JsonAbi,
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
        otc_custody_abi: JsonAbi,
    ) -> Self {
        Self {
            connector_address,
            spoke_pool_address,
            otc_custody_address,
            otc_custody_abi,
        }
    }

    pub fn new_with_default_abis(
        connector_address: Address,
        spoke_pool_address: Address,
        otc_custody_address: Address,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let otc_custody_abi = serde_json::from_str(OTC_CUSTODY_ABI)?;
        Ok(Self::new(
            connector_address,
            spoke_pool_address,
            otc_custody_address,
            otc_custody_abi,
        ))
    }

    pub fn get_across_connector_abi() -> Result<JsonAbi, Box<dyn std::error::Error>> {
        Ok(serde_json::from_str(ACROSS_CONNECTOR_ABI)?)
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

    pub fn encode_deposit_calldata(across_deposit: AcrossDeposit) -> Vec<u8> {
        // Function signature: deposit(address,address,uint256,uint256,uint256,address,uint32,uint32,bytes)
        let function_signature =
            "deposit(address,address,uint256,uint256,uint256,address,uint32,uint32,bytes)";
        let selector = alloy::primitives::keccak256(function_signature.as_bytes())[..4].to_vec();

        // Encode parameters as a tuple matching the Solidity signature:
        // (
        //  address inputToken,
        //  address outputToken,
        //  uint256 depositAmount,
        //  uint256 outputAmount,
        //  uint256 destinationChainId,
        //  address exclusiveRelayer,
        //  uint32 fillDeadline,
        //  uint32 exclusivityDeadline,
        //  bytes  message
        // )

        let encoded_params = SolValue::abi_encode(&(
            across_deposit.input_token,
            across_deposit.output_token,
            across_deposit.deposit_amount,
            across_deposit.output_amount,
            U256::from(across_deposit.destination_chain_id),
            across_deposit.exclusive_relayer,
            across_deposit.fill_deadline as u32,
            across_deposit.exclusivity_deadline as u32,
            "0x".to_string(),
        ));

        // Concatenate selector and encoded parameters
        let mut calldata = selector;
        calldata.extend(encoded_params);
        calldata
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_across_deposit_creation() {
        let connector_address = Address::from_str(ACROSS_CONNECTOR_ADDRESS).unwrap();
        let spoke_pool_address = Address::from_str(ACROSS_SPOKE_POOL_ADDRESS).unwrap();
        let otc_custody_address = Address::from_str(OTC_CUSTODY_ADDRESS).unwrap();
        let otc_custody_abi = serde_json::from_str(OTC_CUSTODY_ABI).unwrap();

        let across_deposit = AcrossDepositBuilder::new(
            connector_address,
            spoke_pool_address,
            otc_custody_address,
            otc_custody_abi,
        );

        assert_eq!(across_deposit.connector_address, connector_address);
        assert_eq!(across_deposit.spoke_pool_address, spoke_pool_address);
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

        assert!(!calldata.is_empty());
        assert_eq!(calldata.len(), 4 + 32 * 8); // selector + 8 parameters * 32 bytes each
    }
}
