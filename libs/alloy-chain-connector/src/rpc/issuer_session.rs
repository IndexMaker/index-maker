use std::{sync::Arc, time::Duration};

use alloy::providers::{Provider, WalletProvider};

use otc_custody::{custody_client::CustodyClientMethods, index::index::IndexInstance};
use symm_core::core::{bits::Amount, functional::OneShotPublishSingle};
use tokio::time::sleep;

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
    account_name: String,
}

impl<P> RpcIssuerSession<P>
where
    P: Provider + WalletProvider,
{
    pub fn new(account_name: String, provider: P) -> Self {
        Self {
            account_name,
            provider,
        }
    }

    pub async fn send_issuer_command(
        &self,
        index: Arc<IndexInstance>,
        command: IssuerCommand,
    ) -> eyre::Result<()> {
        let provider = &self.provider;
        let from_address = provider.default_signer_address();

        match command {
            IssuerCommand::SetSolverWeights {
                basket,
                price,
                observer,
            } => {
                let collateral_token_converter =
                    AmountConverter::new(index.get_collateral_token_precision());

                let price = collateral_token_converter.from_amount(price)?;
                let weights = bytes_from_weights(basket);

                let receipt = index
                    .solver_weights_set_from(provider, from_address, &weights, price)
                    .await?;

                let gas_amount = compute_gas_used(receipt)?;
                observer.one_shot_publish_single(gas_amount);
            }
            IssuerCommand::MintIndex {
                receipient,
                amount,
                seq_num_execution_report,
                observer,
            } => {
                tracing::info!(
                    account_name = %self.account_name,
                    "Minting {} of Index for {}", amount, receipient);

                let index_token_converter = AmountConverter::new(index.get_index_token_precision());
                let amount = index_token_converter.from_amount(amount)?;

                let mut total_gas_amount = Amount::ZERO;
                let num_retries = 3;
                let backoff_seconds = 2;

                // TODO: Design better retry mechanism
                for i in 0..num_retries {
                    let receipt = match index
                        .mint_index_from(
                            provider,
                            from_address,
                            receipient,
                            amount,
                            seq_num_execution_report,
                        )
                        .await
                    {
                        Ok(r) => r,
                        Err(e) => {
                            if i == num_retries - 1 {
                                Err(e)?
                            } else {
                                sleep(Duration::from_secs(backoff_seconds)).await;
                                tracing::warn!(account_name= %self.account_name, "⚠️ Retrying mint index");
                                continue;
                            }
                        }
                    };

                    let tx = receipt.transaction_hash;
                    let gas_amount = compute_gas_used(receipt)?;

                    total_gas_amount += gas_amount;

                    tracing::info!(
                        account_name = %self.account_name,
                        "💳 Minted Index for {} gas used {} tx: {}", receipient, gas_amount, tx);

                    observer.one_shot_publish_single(gas_amount);
                    break;
                }
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
