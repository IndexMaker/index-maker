use alloy::providers::Provider;
use serde_json::json;

// Get current block timestamp
pub async fn get_current_timestamp<P: Provider>(
    provider: &P,
) -> Result<u64, Box<dyn std::error::Error>> {
    let block = provider.get_block_number().await?;
    let block_info = provider.get_block(block.into()).await?.unwrap();
    let timestamp = block_info.header.timestamp;
    println!("  🔗 Current block timestamp: {}", timestamp);
    Ok(timestamp)
}

// Set next block timestamp using Anvil's RPC method
pub async fn set_next_block_timestamp<P: Provider>(
    _provider: &P,
    timestamp: u64,
) -> Result<(), Box<dyn std::error::Error>> {
    // For Anvil nodes, use direct RPC call
    let client = reqwest::Client::new();
    let rpc_url = "http://localhost:8545"; // Default Anvil URL

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
        return Err(format!("RPC error: {}", error).into());
    }
    
    println!("  ✅ Successfully set next block timestamp to: {}", timestamp);
    Ok(())
}
