use alloy::providers::{Provider, WalletProvider};
use alloy_primitives::Address;
use symm_core::core::functional::OneShotPublishSingle;

use otc_custody::contracts::ERC20;

use crate::{
    command::BasicCommand,
    util::{amount_converter::AmountConverter, gas_util::compute_gas_used},
};

pub struct RpcBasicSession<P>
where
    P: Provider + WalletProvider,
{
    provider: P,
    account_name: String,
}

impl<P> RpcBasicSession<P>
where
    P: Provider + WalletProvider,
{
    pub fn new(account_name: String, provider: P) -> Self {
        Self {
            account_name,
            provider,
        }
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
                observer.one_shot_publish_single(balance);
            }
            BasicCommand::Transfer {
                receipient,
                amount,
                observer,
            } => {
                tracing::info!(
                    account_name = %self.account_name,
                    "Transferring collateral {} from wallet to {}",
                    amount,
                    receipient
                );

                let signer_address = provider.default_signer_address();
                let sender_balance_raw = contract.balanceOf(signer_address).call().await?;
                let sender_balance = converter.into_amount(sender_balance_raw)?;
                let receipient_balance_raw = contract.balanceOf(receipient).call().await?;
                let receipient_balance = converter.into_amount(receipient_balance_raw)?;
                tracing::info!(
                    account_name = %self.account_name,
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

                let tx = receipt.transaction_hash;
                let gas_amount = compute_gas_used(receipt)?;

                tracing::info!(
                    account_name = %self.account_name,
                    "💰 Collateral transferred to wallet {} gas used {} tx {}",
                    receipient,
                    gas_amount,
                    tx
                );

                observer.one_shot_publish_single(gas_amount);

                let sender_balance_raw = contract.balanceOf(signer_address).call().await?;
                let sender_balance = converter.into_amount(sender_balance_raw)?;
                let receipient_balance_raw = contract.balanceOf(receipient).call().await?;
                let receipient_balance = converter.into_amount(receipient_balance_raw)?;
                tracing::info!(
                    account_name = %self.account_name,
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
