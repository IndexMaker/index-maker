use alloy::providers::Provider;
use alloy::primitives::{utils::format_units, U256};
use rust_decimal::Decimal;
use rust_decimal::prelude::ToPrimitive;
use serde_json::json;
use std::str::FromStr;
use symm_core::core::bits::Amount;
use crate::config::EvmConnectorConfig;

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

/// Convert Amount (decimal USDC) to U256 (wei format for 6 decimals)
/// Example: Amount::from("100.50") -> U256::from(100_500_000u64)
pub fn amount_to_u256(amount: &Amount) -> eyre::Result<U256> {
    let decimal_str = amount.to_string();
    let decimal = Decimal::from_str(&decimal_str)
        .map_err(|e| eyre::eyre!("Failed to parse amount as decimal: {}", e))?;
    
    // Convert to USDC wei (6 decimals)
    let usdc_wei = decimal * Decimal::from(1_000_000);
    let usdc_wei_u64 = usdc_wei
        .to_u64()
        .ok_or_else(|| eyre::eyre!("Amount too large to convert to U256"))?;
    
    Ok(U256::from(usdc_wei_u64))
}

/// Convert U256 (wei format for 6 decimals) to Amount (decimal USDC)  
/// Example: U256::from(100_500_000u64) -> Amount::from("100.50")
pub fn u256_to_amount(wei_amount: U256) -> eyre::Result<Amount> {
    let formatted = format_units(wei_amount, 6)
        .map_err(|e| eyre::eyre!("Failed to format U256 to units: {}", e))?;
    
    Amount::try_from(formatted.as_str())
        .map_err(|e| eyre::eyre!("Failed to convert formatted string to Amount: {}", e))
}

/// Convert Decimal to U256 (wei format for 6 decimals)
/// Example: Decimal::from_str("100.50") -> U256::from(100_500_000u64)  
pub fn decimal_to_u256(decimal: Decimal) -> eyre::Result<U256> {
    let usdc_wei = decimal * Decimal::from(1_000_000);
    let usdc_wei_u64 = usdc_wei
        .to_u64()
        .ok_or_else(|| eyre::eyre!("Decimal too large to convert to U256"))?;
    
    Ok(U256::from(usdc_wei_u64))
}

/// Convert U256 (wei format for 6 decimals) to Decimal
/// Example: U256::from(100_500_000u64) -> Decimal::from_str("100.50")
pub fn u256_to_decimal(wei_amount: U256) -> eyre::Result<Decimal> {
    let formatted = format_units(wei_amount, 6)
        .map_err(|e| eyre::eyre!("Failed to format U256 to units: {}", e))?;
    
    Decimal::from_str(&formatted)
        .map_err(|e| eyre::eyre!("Failed to parse formatted string as decimal: {}", e))
}

/// Get gas price in USDC as Amount (simplified calculation)
/// Uses a fixed ETH to USDC rate of $3000/ETH for simplicity
pub async fn get_gas_price_usdc<P: Provider>(provider: &P) -> eyre::Result<Amount> {
    let gas_price_wei = provider.get_gas_price().await
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
pub async fn calculate_gas_fee_usdc<P: Provider>(provider: &P, gas_used: U256) -> eyre::Result<Amount> {
    let gas_price_usdc = get_gas_price_usdc(provider).await?;
    let gas_price_u256 = amount_to_u256(&gas_price_usdc)?;
    
    let total_fee_wei = gas_price_u256 * gas_used;
    u256_to_amount(total_fee_wei)
}
