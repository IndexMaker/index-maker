use alloy::providers::{Provider, WalletProvider};
use alloy_primitives::Address;
use symm_core::core::functional::PublishSingle;

use crate::{
    command::CustodyCommand,
    contracts::OTCCustody,
    util::{
        amount_converter::AmountConverter,
        gas_util::compute_gas_used,
        verification_data::{build_id, build_verification_data},
    },
};

pub struct RpcCustodySession<P>
where
    P: Provider + WalletProvider,
{
    provider: P,
    signer_address: Address,
    converter: AmountConverter,
}

impl<P> RpcCustodySession<P>
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

    pub async fn send_custody_command(&self, command: CustodyCommand) -> eyre::Result<()> {
        let provider = &self.provider;
        let custody = OTCCustody::new(self.signer_address, provider);

        match command {
            CustodyCommand::AddressToCustody {
                custody_id: id,
                token,
                amount,
                observer,
            } => {
                let id = build_id(id);
                let amount = self.converter.from_amount(amount)?;

                let receipt = custody
                    .addressToCustody(id, token, amount)
                    .send()
                    .await?
                    .get_receipt()
                    .await?;

                let gas_amount = compute_gas_used(&self.converter, receipt)?;
                observer.publish_single(gas_amount);
            }
            CustodyCommand::CustodyToAddress {
                token,
                destination,
                amount,
                observer,
            } => {
                let amount = self.converter.from_amount(amount)?;
                let verification_data = build_verification_data();

                let receipt = custody
                    .custodyToAddress(token, destination, amount, verification_data)
                    .send()
                    .await?
                    .get_receipt()
                    .await?;

                let gas_amount = compute_gas_used(&self.converter, receipt)?;
                observer.publish_single(gas_amount);
            }
            CustodyCommand::GetCustodyBalances {
                custody_id: id,
                token,
                observer,
            } => {
                let id = build_id(id);
                let balance = custody.getCustodyBalances(id, token).call().await?;
                let balance = self.converter.into_amount(balance)?;
                observer.publish_single(balance);
            }
        }

        Ok(())
    }
}
