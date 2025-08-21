use alloy::providers::{Provider, WalletProvider};
use alloy_primitives::Address;
use symm_core::core::functional::PublishSingle;

use crate::{
    command::BasicCommand,
    contracts::ERC20,
    util::{amount_converter::AmountConverter, gas_util::compute_gas_used},
};

pub struct RpcBasicSession<P>
where
    P: Provider + WalletProvider,
{
    provider: P,
    signer_address: Address,
    converter: AmountConverter,
}

impl<P> RpcBasicSession<P>
where
    P: Provider + WalletProvider,
{
    pub async fn try_new(provider: P, signer_address: Address) -> eyre::Result<Self> {
        let erc20 = ERC20::new(signer_address, &provider);
        let decimals = erc20.decimals().call().await?;
        let converter = AmountConverter::new(decimals);

        Ok(Self {
            provider,
            signer_address,
            converter,
        })
    }

    pub fn get_amount_converter(&self) -> &AmountConverter {
        &self.converter
    }

    pub async fn send_basic_command(&self, command: BasicCommand) -> eyre::Result<()> {
        let provider = &self.provider;
        let erc20 = ERC20::new(self.signer_address, provider);

        match command {
            BasicCommand::BalanceOf { account, observer } => {
                let balance = erc20.balanceOf(account).call().await?;
                let balance = self.converter.into_amount(balance)?;
                observer.publish_single(balance);
            }
            BasicCommand::Transfer {
                to,
                amount,
                observer,
            } => {
                let amount = self.converter.from_amount(amount)?;
                let receipt = erc20
                    .transfer(to, amount)
                    .send()
                    .await?
                    .get_receipt()
                    .await?;

                let gas_amount = compute_gas_used(&self.converter, receipt)?;
                observer.publish_single(gas_amount);
            }
        }
        Ok(())
    }
}
