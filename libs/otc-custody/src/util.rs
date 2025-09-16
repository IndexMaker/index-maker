use alloy::{
    consensus::BlockHeader,
    eips::BlockId,
    network::BlockResponse,
    primitives::{Address, U256},
    providers::Provider,
    rpc::types::{Block, BlockNumberOrTag},
};
use eyre::OptionExt;

pub async fn with_last_block<R>(
    provider: &impl Provider,
    cb: impl Fn(Block) -> R,
) -> eyre::Result<R> {
    let ret = provider
        .get_block_by_number(BlockNumberOrTag::Latest)
        .await?
        .map(cb)
        .ok_or_eyre("Block not found")?;

    Ok(ret)
}

pub async fn get_last_block_timestamp_and_base_gas_fee(
    provider: &impl Provider,
) -> eyre::Result<(U256, Option<u64>)> {
    let (timestamp, base_gas_fee) = with_last_block(&provider, |b| {
        (U256::from(b.header.timestamp), b.header.base_fee_per_gas)
    })
    .await?;

    Ok((timestamp, base_gas_fee))
}

pub async fn get_last_block_timestamp(provider: &impl Provider) -> eyre::Result<U256> {
    let timestamp = provider
        .get_block_by_number(BlockNumberOrTag::Latest)
        .await?
        .map(|b| U256::from(b.header().timestamp()))
        .ok_or_eyre("Block not found")?;

    Ok(timestamp)
}

pub async fn pending_nonce<P: Provider>(p: &P, from: Address) -> eyre::Result<u64> {
    let n: u64 = p
        .get_transaction_count(from)
        .block_id(BlockId::Number(BlockNumberOrTag::Pending))
        .await?;
    Ok(n)
}
