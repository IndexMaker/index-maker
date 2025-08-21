use alloy::providers::{Provider, WalletProvider};
use symm_core::core::{bits::Address, functional::PublishSingle};

use crate::{
    command::IssuerCommand,
    contracts::OTCIndex,
    util::{
        amount_converter::AmountConverter, gas_util::compute_gas_used,
        timestamp_util::timestamp_from_date, verification_data::build_verification_data,
        weights_util::bytes_from_weights,
    },
};

pub struct RpcIssuerSession<P>
where
    P: Provider + WalletProvider,
{
    provider: P,
    signer_address: Address,
    converter: AmountConverter,
}

impl<P> RpcIssuerSession<P>
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

    pub async fn send_issuer_command(&self, command: IssuerCommand) -> eyre::Result<()> {
        let provider = &self.provider;
        let index = OTCIndex::new(self.signer_address, provider);

        match command {
            IssuerCommand::SetSolverWeights {
                basket,
                price,
                timestamp,
                observer,
            } => {
                let timestamp = timestamp_from_date(timestamp);
                let price = self.converter.from_amount(price)?;
                let weights = bytes_from_weights(basket);

                let receipt = index
                    .solverUpdate(timestamp, weights, price)
                    .send()
                    .await?
                    .get_receipt()
                    .await?;

                let gas_amount = compute_gas_used(&self.converter, receipt)?;
                observer.publish_single(gas_amount);
            }
            IssuerCommand::MintIndex {
                target,
                amount,
                seq_num_execution_report,
                observer,
            } => {
                let amount = self.converter.from_amount(amount)?;

                let receipt = index
                    .mint(target, amount, seq_num_execution_report)
                    .send()
                    .await?
                    .get_receipt()
                    .await?;

                let gas_amount = compute_gas_used(&self.converter, receipt)?;
                observer.publish_single(gas_amount);
            }
            IssuerCommand::BurnIndex {
                target,
                amount,
                seq_num_new_order_single,
                observer,
            } => {
                let amount = self.converter.from_amount(amount)?;

                let receipt = index
                    .burn(amount, target, seq_num_new_order_single)
                    .send()
                    .await?
                    .get_receipt()
                    .await?;

                let gas_amount = compute_gas_used(&self.converter, receipt)?;
                observer.publish_single(gas_amount);
            }
            IssuerCommand::Withdraw {
                receipient,
                amount,
                execution_report,
                observer,
            } => {
                let amount = self.converter.from_amount(amount)?;
                let verification_data = build_verification_data();

                let receipt = index
                    .withdraw(amount, receipient, verification_data, execution_report)
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
