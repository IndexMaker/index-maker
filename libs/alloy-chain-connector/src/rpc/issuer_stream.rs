use alloy::providers::{Provider, WalletProvider};
use index_core::blockchain::chain_connector::ChainNotification;
use symm_core::core::bits::Address;

use crate::util::amount_converter::AmountConverter;

pub struct RpcIssuerStream<P>
where
    P: Provider + WalletProvider,
{
    provider: P,
    signer_address: Address,
    converter: AmountConverter,
}

impl<P> RpcIssuerStream<P>
where
    P: Provider + WalletProvider,
{
    pub fn new(provider: P, signer_address: Address, converter: AmountConverter) -> Self {
        Self {
            provider,
            signer_address,
            converter,
        }
    }

    pub async fn next_event(&self) -> eyre::Result<ChainNotification> {
        let _ = &self.provider;
        let _ = &self.signer_address;
        let _ = &self.converter;
        todo!()
    }
}
