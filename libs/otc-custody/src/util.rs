use alloy::{
    consensus::BlockHeader, network::BlockResponse, primitives::U256, providers::Provider,
    rpc::types::BlockNumberOrTag,
};
use eyre::OptionExt;

pub async fn get_last_block_timestamp(provider: &impl Provider) -> eyre::Result<U256> {
    let timestamp = provider
        .get_block_by_number(BlockNumberOrTag::Latest)
        .await?
        .map(|b| U256::from(b.header().timestamp()))
        .ok_or_eyre("Block not found")?;

    Ok(timestamp)
}
