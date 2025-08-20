use alloy::providers::{Provider, WalletProvider};
use alloy_primitives::U256;
use symm_core::core::functional::PublishSingle;

use crate::{command::CustodyCommand, contracts::OTCCustody};

pub struct RpcCustodySession<P>
where
    P: Provider + WalletProvider,
{
    provider: P,
}

impl<P> RpcCustodySession<P>
where
    P: Provider + WalletProvider,
{
    pub fn new(provider: P) -> Self {
        Self { provider }
    }

    pub async fn send_custody_command(&self, command: CustodyCommand) -> eyre::Result<()> {
        let signer_address = self.provider.default_signer_address();
        let provider = &self.provider;
        let custody = OTCCustody::new(signer_address, provider);

        match command {
            CustodyCommand::AddressToCustody {
                id,
                token,
                amount,
                observer,
            } => {
                custody.addressToCustody(id, token, amount).call().await?;

                let fee = U256::from(0);
                observer.publish_single(fee);
            }
            CustodyCommand::CustodyToAddress {
                token,
                destination,
                amount,
                verification_data,
                observer,
            } => {
                custody
                    .custodyToAddress(token, destination, amount, verification_data)
                    .call()
                    .await?;

                let fee = U256::from(0);
                observer.publish_single(fee);
            }
            CustodyCommand::GetCustodyBalances {
                id,
                token,
                observer,
            } => {
                let res = custody.getCustodyBalances(id, token).call().await?;
                observer.publish_single(res);
            }
        }

        Ok(())
    }
}
