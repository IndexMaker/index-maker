use alloy::providers::{Provider, WalletProvider};
use futures::future::pending;
use index_core::blockchain::chain_connector::ChainNotification;
use symm_core::core::bits::Address;

use crate::{contracts::ERC20, util::amount_converter::AmountConverter};

pub struct RpcIssuerStream<P>
where
    P: Provider + WalletProvider,
{
    provider: P,
}

impl<P> RpcIssuerStream<P>
where
    P: Provider + WalletProvider,
{
    pub fn new(provider: P) -> Self {
        Self { provider }
    }

    pub async fn next_event(&self, usdc_address: Address) -> eyre::Result<ChainNotification> {
        let provider = &self.provider;
        let usdc = ERC20::new(usdc_address, provider);
        let decimals = usdc.decimals().call().await?;
        let converter = AmountConverter::new(decimals);
        let _ = converter;
        pending::<()>().await;
        todo!()
    }
}
