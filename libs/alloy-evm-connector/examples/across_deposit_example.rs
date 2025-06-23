use alloy::primitives::{address, U256};
use alloy_evm_connector::across_deposit::{
    example_complete_across_deposit_flow, new_builder_from_env, AcrossDepositBuilder,
    ARBITRUM_CHAIN_ID, BASE_CHAIN_ID, USDC_ARBITRUM_ADDRESS, USDC_BASE_ADDRESS,
};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("🚀 Starting Across Deposit Example");

    // Check if environment variables are set
    match std::env::var("PRIVATE_KEY") {
        Ok(_) => println!("✅ PRIVATE_KEY found in environment"),
        Err(_) => {
            println!("❌ PRIVATE_KEY not found in environment");
            println!("Please create a .env file with your private key:");
            println!("PRIVATE_KEY=your_private_key_here");
            println!("RPC_URL=http://localhost:8545");
            return Ok(());
        }
    }

    // Option 1: Use the pre-built example function (simplest)
    println!("\n📋 Running pre-built example...");
    match example_complete_across_deposit_flow().await {
        Ok(_) => println!("✅ Pre-built example completed successfully!"),
        Err(e) => println!("❌ Pre-built example failed: {}", e),
    }

    // Option 2: Create builder manually and execute specific steps
    println!("\n🔧 Running manual example...");
    match run_manual_example().await {
        Ok(_) => println!("✅ Manual example completed successfully!"),
        Err(e) => println!("❌ Manual example failed: {}", e),
    }

    println!("\n🎉 All examples completed!");
    Ok(())
}

async fn run_manual_example() -> Result<(), Box<dyn std::error::Error>> {
    // Create builder using environment variables
    let builder = new_builder_from_env().await?;

    // Example parameters
    let recipient = address!("0xC0D3CB2E7452b8F4e7710bebd7529811868a85dd");
    let input_token = USDC_ARBITRUM_ADDRESS;
    let output_token = USDC_BASE_ADDRESS;
    let deposit_amount = U256::from(1_000_000u128); // 1 USDC (6 decimals)
    let origin_chain_id = ARBITRUM_CHAIN_ID;
    let destination_chain_id = BASE_CHAIN_ID;

    println!("  📊 Parameters:");
    println!("    Recipient: {}", recipient);
    println!("    Input Token: {}", input_token);
    println!("    Output Token: {}", output_token);
    println!(
        "    Amount: {} ({} USDC)",
        deposit_amount,
        deposit_amount / U256::from(1_000_000u128)
    );
    println!("    Origin Chain: {}", origin_chain_id);
    println!("    Destination Chain: {}", destination_chain_id);

    // Execute the complete flow
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

    Ok(())
}
