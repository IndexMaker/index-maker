use crate::config::EvmConnectorConfig;
use alloy::primitives::{
    utils::{format_units, parse_units},
    U256,
};
use alloy::providers::Provider;
use rust_decimal::Decimal;
use serde_json::json;
use std::str::FromStr;
use symm_core::core::bits::Amount;

/// Trait for converting types to EVM-compatible U256 amounts
pub trait IntoEvmAmount {
    /// Convert to U256 using specified decimal places
    fn into_evm_amount(&self, decimals: u8) -> eyre::Result<U256>;

    /// Convert to U256 using USDC decimals (6) as default
    fn into_evm_amount_usdc(&self) -> eyre::Result<U256> {
        self.into_evm_amount(6)
    }

    /// Convert to U256 using ETH decimals (18)
    fn into_evm_amount_with_decimals(&self, decimals: u8) -> eyre::Result<U256> {
        self.into_evm_amount(decimals)
    }

    fn into_evm_amount_default(&self) -> eyre::Result<U256> {
        self.into_evm_amount(0)
    }
}

/// Trait for converting U256 to Amount types
pub trait IntoAmount {
    /// Convert to Amount using specified decimal places
    fn into_amount(&self, decimals: u8) -> eyre::Result<Amount>;

    /// Convert to Amount using USDC decimals (6) as default
    fn into_amount_usdc(&self) -> eyre::Result<Amount> {
        self.into_amount(6)
    }
}

/// Implementation for Amount -> U256 conversion
impl IntoEvmAmount for Amount {
    fn into_evm_amount(&self, decimals: u8) -> eyre::Result<U256> {
        amount_to_u256_with_decimals(self, decimals)
    }
}

// Note: Since Amount is a type alias for Decimal, this impl works for both
// You can use dec!(10.0).into_evm_amount() or amount_var.into_evm_amount()

impl IntoEvmAmount for u32 {
    fn into_evm_amount(&self, _decimals: u8) -> eyre::Result<U256> {
        Ok(U256::from(*self))
    }
}

/// Implementation for numeric types -> U256 conversion
impl IntoEvmAmount for u64 {
    fn into_evm_amount(&self, _decimals: u8) -> eyre::Result<U256> {
        // For raw numeric types, decimals parameter is ignored - they represent raw wei values
        Ok(U256::from(*self))
    }
}

impl IntoEvmAmount for u128 {
    fn into_evm_amount(&self, _decimals: u8) -> eyre::Result<U256> {
        // For raw numeric types, decimals parameter is ignored - they represent raw wei values
        Ok(U256::from(*self))
    }
}

/// Implementation for U256 -> Amount conversion
impl IntoAmount for U256 {
    fn into_amount(&self, decimals: u8) -> eyre::Result<Amount> {
        u256_to_amount_with_decimals(*self, decimals)
    }
}

// Get current block timestamp
pub async fn get_current_timestamp<P: Provider>(provider: &P) -> eyre::Result<u64> {
    let block = provider.get_block_number().await?;
    let block_info = provider.get_block(block.into()).await?.unwrap();
    let timestamp = block_info.header.timestamp;
    Ok(timestamp)
}

// Set next block timestamp using Anvil's RPC method
pub async fn set_next_block_timestamp<P: Provider>(
    _provider: &P,
    timestamp: u64,
) -> eyre::Result<()> {
    // For Anvil nodes, use direct RPC call
    let client = reqwest::Client::new();
    let rpc_url = &EvmConnectorConfig::get_default_rpc_url();

    let response = client
        .post(rpc_url)
        .json(&json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "anvil_setNextBlockTimestamp",
            "params": [timestamp]
        }))
        .send()
        .await?;

    let result: serde_json::Value = response.json().await?;

    if let Some(error) = result.get("error") {
        return Err(eyre::eyre!("RPC error: {}", error));
    }

    Ok(())
}

/// Convert Amount to U256 with specified decimal places using alloy's parse_units
/// Example: Amount::from("100.50"), 6 -> U256::from(100_500_000u64) (USDC format)
/// Example: Amount::from("1.0"), 18 -> U256::from(1_000_000_000_000_000_000u64) (ETH format)
pub fn amount_to_u256_with_decimals(amount: &Amount, decimals: u8) -> eyre::Result<U256> {
    let amount_str = amount.to_string();

    let parsed_units = parse_units(&amount_str, decimals).map_err(|e| {
        eyre::eyre!(
            "Failed to parse amount '{}' with {} decimals: {}",
            amount_str,
            decimals,
            e
        )
    })?;

    // Convert ParseUnits to U256
    Ok(parsed_units.into())
}

/// Convert Amount to U256 (defaults to 6 decimals for USDC compatibility)
/// Example: Amount::from("100.50") -> U256::from(100_500_000u64)
pub fn amount_to_u256(amount: &Amount) -> eyre::Result<U256> {
    amount_to_u256_with_decimals(amount, 6)
}

/// Convert U256 to Amount with specified decimal places
/// Example: U256::from(100_500_000u64), 6 -> Amount::from("100.50") (USDC format)  
/// Example: U256::from(1_000_000_000_000_000_000u64), 18 -> Amount::from("1.0") (ETH format)
pub fn u256_to_amount_with_decimals(wei_amount: U256, decimals: u8) -> eyre::Result<Amount> {
    let formatted = format_units(wei_amount, decimals).map_err(|e| {
        eyre::eyre!(
            "Failed to format U256 to units with {} decimals: {}",
            decimals,
            e
        )
    })?;

    Amount::try_from(formatted.as_str())
        .map_err(|e| eyre::eyre!("Failed to convert formatted string to Amount: {}", e))
}

/// Convert U256 to Amount (defaults to 6 decimals for USDC compatibility)
/// Example: U256::from(100_500_000u64) -> Amount::from("100.50")
pub fn u256_to_amount(wei_amount: U256) -> eyre::Result<Amount> {
    u256_to_amount_with_decimals(wei_amount, 6)
}

// Note: These internal functions still use the underlying Decimal type
// since Amount is just a type alias for Decimal. The public API uses Amount.

/// Get gas price in USDC as Amount (simplified calculation)
/// Uses a fixed ETH to USDC rate of $3000/ETH for simplicity
pub async fn get_gas_price_usdc<P: Provider>(provider: &P) -> eyre::Result<Amount> {
    let gas_price_wei = provider
        .get_gas_price()
        .await
        .map_err(|e| eyre::eyre!("Failed to get gas price: {}", e))?;

    // Convert gas price from wei to ETH
    let gas_price_eth_str = format_units(gas_price_wei, "ether")
        .map_err(|e| eyre::eyre!("Failed to format gas price: {}", e))?;

    let gas_price_eth = Decimal::from_str(&gas_price_eth_str)
        .map_err(|e| eyre::eyre!("Failed to parse ETH gas price: {}", e))?;

    // Convert ETH to USDC using configured rate
    let eth_to_usdc_rate = Decimal::from(EvmConnectorConfig::get_eth_to_usdc_rate());
    let gas_price_usdc = gas_price_eth * eth_to_usdc_rate;

    Amount::try_from(gas_price_usdc.to_string().as_str())
        .map_err(|e| eyre::eyre!("Failed to convert gas price to Amount: {}", e))
}

/// Calculate total gas fee in USDC given gas used
pub async fn calculate_gas_fee_usdc<P: Provider>(
    provider: &P,
    gas_used: U256,
) -> eyre::Result<Amount> {
    let gas_price_usdc = get_gas_price_usdc(provider).await?;
    let gas_price_u256 = gas_price_usdc.into_evm_amount(6)?;

    let total_fee_wei = gas_price_u256 * gas_used;
    total_fee_wei.into_amount(6)
}
