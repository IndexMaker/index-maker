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
}

impl<P> RpcBasicSession<P>
where
    P: Provider + WalletProvider,
{
    pub fn new(provider: P) -> Self {
        Self { provider }
    }

    pub async fn send_basic_command(
        &self,
        contract_address: Address,
        command: BasicCommand,
    ) -> eyre::Result<()> {
        let provider = &self.provider;
        let contract = ERC20::new(contract_address, provider);
        let decimals = contract.decimals().call().await?;
        let converter = AmountConverter::new(decimals);

        match command {
            BasicCommand::BalanceOf { account, observer } => {
                let balance = contract.balanceOf(account).call().await?;
                let balance = converter.into_amount(balance)?;
                observer.publish_single(balance);
            }
            BasicCommand::Transfer {
                receipient,
                amount,
                observer,
            } => {
                let signer_address = provider.default_signer_address();
                let sender_balance_raw = contract.balanceOf(signer_address).call().await?;
                let sender_balance = converter.into_amount(sender_balance_raw)?;
                let receipient_balance_raw = contract.balanceOf(receipient).call().await?;
                let receipient_balance = converter.into_amount(receipient_balance_raw)?;
                tracing::info!(
                    %contract_address,
                    %signer_address,
                    %receipient,
                    %sender_balance,
                    %sender_balance_raw,
                    %receipient_balance,
                    %receipient_balance_raw,
                    %amount,
                    "Send Transfer");

                let amount_raw = converter.from_amount(amount)?;
                let receipt = contract
                    .transfer(receipient, amount_raw)
                    .send()
                    .await?
                    .get_receipt()
                    .await?;

                let gas_amount = compute_gas_used(&converter, receipt)?;
                observer.publish_single(gas_amount);
                
                let sender_balance_raw = contract.balanceOf(signer_address).call().await?;
                let sender_balance = converter.into_amount(sender_balance_raw)?;
                let receipient_balance_raw = contract.balanceOf(receipient).call().await?;
                let receipient_balance = converter.into_amount(receipient_balance_raw)?;
                tracing::info!(
                    %contract_address,
                    %signer_address,
                    %receipient,
                    %sender_balance,
                    %sender_balance_raw,
                    %receipient_balance,
                    %receipient_balance_raw,
                    %amount,
                    %amount_raw,
                    %gas_amount,
                    "Transfer Complete");
            }
        }
        Ok(())
    }
}
