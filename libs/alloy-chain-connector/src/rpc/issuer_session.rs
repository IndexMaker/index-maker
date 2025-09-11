use std::{sync::Arc, time::Duration};

use alloy::providers::{Provider, WalletProvider};

use eyre::OptionExt;
use itertools::Itertools;
use otc_custody::{custody_client::CustodyClientMethods, index::index::IndexInstance};
use symm_core::core::{bits::Amount, functional::OneShotPublishSingle};
use tokio::time::sleep;

use crate::{
    command::IssuerCommand,
    credentials::MultiProvider,
    util::{
        amount_converter::AmountConverter, gas_util::compute_gas_used,
        weights_util::bytes_from_weights,
    },
};

pub struct RpcIssuerSession<P>
where
    P: Provider + WalletProvider + Clone + 'static,
{
    providers: MultiProvider<P>,
    account_name: String,
}

impl<P> RpcIssuerSession<P>
where
    P: Provider + WalletProvider + Clone + 'static,
{
    pub fn new(account_name: String, providers: MultiProvider<P>) -> Self {
        Self {
            account_name,
            providers,
        }
    }

    pub async fn send_issuer_command(
        &mut self,
        index: Arc<IndexInstance>,
        command: IssuerCommand,
    ) -> eyre::Result<()> {
        let (num_retries, backoff_period) = self
            .providers
            .with_shared_date(|s| (s.max_retries, s.retry_backoff));

        let mut current_n = self.providers.next_n_providers(2).await.current_n(2);
        let mut current = current_n.first().ok_or_eyre("No provider")?;

        let (mut provider, mut rpc_url) = (&current.0, &current.1);

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
                    %rpc_url,
                    "Minting {} of Index for {}", amount, receipient);

                let index_token_converter = AmountConverter::new(index.get_index_token_precision());
                let amount = index_token_converter.from_amount(amount)?;

                let mut total_gas_amount = Amount::ZERO;

                for i in 0..num_retries {
                    let providers = current_n.iter().map(|(p,_)| p).collect_vec();
                    let receipt = match index
                        .mint_index_from(
                            &providers,
                            from_address,
                            receipient,
                            amount,
                            seq_num_execution_report,
                        )
                        .await
                    {
                        Ok(r) => r,
                        Err(e) => {
                            current_n = self.providers.next_n_providers(2).await.current_n(2);
                            current = current_n.first().ok_or_eyre("No provider")?;
                            rpc_url = &current.1;

                            if i == num_retries - 1 {
                                Err(e)?
                            } else {
                                sleep(backoff_period).await;
                                tracing::warn!(
                                    account_name= %self.account_name, 
                                    %rpc_url,
                                    "⚠️ Retrying mint index");
                                continue;
                            }
                        }
                    };

                    let tx = receipt.transaction_hash;
                    let gas_amount = compute_gas_used(receipt)?;

                    total_gas_amount += gas_amount;

                    tracing::info!(
                        account_name = %self.account_name,
                        %rpc_url,
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
