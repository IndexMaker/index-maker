use alloy::providers::{Provider, WalletProvider};
use index_core::blockchain::chain_connector::ChainNotification;


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

    pub async fn next_event(&self) -> eyre::Result<ChainNotification> {
        let _ = &self.provider;
        todo!()
    }
}
