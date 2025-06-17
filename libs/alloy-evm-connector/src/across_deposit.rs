use alloy::{
    hex,
    json_abi::JsonAbi,
    primitives::{Address, B256, U256},
    sol_types::SolValue,
};
use serde::{Deserialize, Serialize};
use std::str::FromStr;

pub const ACROSS_CONNECTOR_ADDRESS: &str = "0x16635f631A1D2e96EE7aF937379583139e835859";
pub const ACROSS_SPOKE_POOL_ADDRESS: &str = "0x09aea4b2242abC8bb4BB78D537A67a245A7bEC64";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AcrossSuggestedOutput {
    pub output_amount: U256,
    pub fill_deadline: u64,
    pub exclusive_relayer: Address,
    pub exclusivity_deadline: u64,
}

pub struct AcrossDeposit {
    pub connector_address: Address,
    pub spoke_pool_address: Address,
    pub otc_custody_address: Address,
    pub otc_custody_abi: JsonAbi,
}

impl AcrossDeposit {
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

    pub fn encode_deposit_calldata(
        &self,
        input_token: Address,
        output_token: Address,
        deposit_amount: U256,
        output_amount: U256,
        destination_chain_id: u64,
        exclusive_relayer: Address,
        fill_deadline: u64,
        exclusivity_deadline: u64,
    ) -> Vec<u8> {
        // Function signature: deposit(address,address,uint256,uint256,uint256,address,uint32,uint32,bytes)
        let function_signature =
            "deposit(address,address,uint256,uint256,uint256,address,uint32,uint32,bytes)";
        let selector = alloy::primitives::keccak256(function_signature.as_bytes())[..4].to_vec();

        // Encode parameters
        let encoded_params = SolValue::abi_encode_params(&[
            &input_token,
            &output_token,
            &deposit_amount,
            &output_amount,
            &destination_chain_id,
            &exclusive_relayer,
            &fill_deadline,
            &exclusivity_deadline,
            &"0x".to_string(),
        ]);

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
        let otc_custody_address = Address::ZERO;
        let otc_custody_abi = JsonAbi::default();

        let across_deposit = AcrossDeposit::new(
            connector_address,
            spoke_pool_address,
            otc_custody_address,
            otc_custody_abi,
        );

        assert_eq!(across_deposit.connector_address, connector_address);
        assert_eq!(across_deposit.spoke_pool_address, spoke_pool_address);
    }

    #[test]
    fn test_get_deposit_data() {
        let across_deposit = AcrossDeposit::new(
            Address::ZERO,
            Address::ZERO,
            Address::ZERO,
            JsonAbi::default(),
        );

        let source_token = Address::ZERO;
        let dest_token = Address::ZERO;
        let amount = U256::from(1000000u128);
        let destination_chain_id = 42161u64;
        let recipient = Address::ZERO;
        let origin_chain_id = 1u64;

        let deposit_data = across_deposit.get_deposit_data(
            source_token,
            dest_token,
            amount,
            destination_chain_id,
            recipient,
            origin_chain_id,
        );

        assert_eq!(deposit_data.input_amount, amount);
        assert_eq!(deposit_data.output_amount, amount);
        assert_eq!(deposit_data.origin_chain_id, origin_chain_id);
    }

    #[test]
    fn test_encode_deposit_calldata() {
        let across_deposit = AcrossDeposit::new(
            Address::ZERO,
            Address::ZERO,
            Address::ZERO,
            JsonAbi::default(),
        );

        let calldata = across_deposit.encode_deposit_calldata(
            Address::ZERO,
            Address::ZERO,
            U256::from(1000000u128),
            U256::from(1000000u128),
            42161u64,
            Address::ZERO,
            0u64,
            0u64,
        );

        assert!(!calldata.is_empty());
        assert_eq!(calldata.len(), 4 + 32 * 9); // selector + 9 parameters * 32 bytes each
    }
}
