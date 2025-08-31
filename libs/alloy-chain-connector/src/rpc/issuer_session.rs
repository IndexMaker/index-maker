use std::sync::Arc;

use alloy::providers::{Provider, WalletProvider};
use symm_core::core::functional::PublishSingle;

use otc_custody::{custody_client::CustodyClientMethods, index::index::IndexInstance};

use crate::{
    command::IssuerCommand,
    util::{
        amount_converter::AmountConverter, gas_util::compute_gas_used,
        weights_util::bytes_from_weights,
    },
};

pub struct RpcIssuerSession<P>
where
    P: Provider + WalletProvider,
{
    provider: P,
}

impl<P> RpcIssuerSession<P>
where
    P: Provider + WalletProvider,
{
    pub fn new(provider: P) -> Self {
        Self { provider }
    }

    pub async fn send_issuer_command(
        &self,
        index: Arc<IndexInstance>,
        command: IssuerCommand,
    ) -> eyre::Result<()> {
        let provider = &self.provider;
        let from_address = provider.default_signer_address();
        let decimals = index.get_collateral_token_precision();
        let converter = AmountConverter::new(decimals);

        match command {
            IssuerCommand::SetSolverWeights {
                basket,
                price,
                observer,
            } => {
                let price = converter.from_amount(price)?;
                let weights = bytes_from_weights(basket);

                let receipt = index
                    .solver_weights_set_from(provider, from_address, &weights, price)
                    .await?;

                let gas_amount = compute_gas_used(&converter, receipt)?;
                observer.publish_single(gas_amount);
            }
            IssuerCommand::MintIndex {
                receipient,
                amount,
                seq_num_execution_report,
                observer,
            } => {
                tracing::info!("Minting {} of Index for {}", amount, receipient);

                let amount = converter.from_amount(amount)?;

                let receipt = index
                    .mint_index_from(
                        provider,
                        from_address,
                        receipient,
                        amount,
                        seq_num_execution_report,
                    )
                    .await?;

                let gas_amount = compute_gas_used(&converter, receipt)?;

                tracing::info!("💳 Minted Index for {} gas used {}", receipient, gas_amount);
                
                observer.publish_single(gas_amount);
            }
            IssuerCommand::BurnIndex { .. } => {
                todo!();
            }
            IssuerCommand::Withdraw { .. } => {
                todo!();
            }
        }
        Ok(())
    }
}
