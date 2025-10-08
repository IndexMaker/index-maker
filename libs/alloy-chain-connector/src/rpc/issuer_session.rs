use std::sync::Arc;

use alloy::providers::{Provider, WalletProvider};

use alloy_primitives::U256;
use eyre::OptionExt;
use index_core::index::basket::Basket;
use otc_custody::{custody_client::CustodyClientMethods, index::index::IndexInstance};
use symm_core::core::{
    bits::{Address, Amount},
    functional::{OneShotPublishSingle, OneShotSingleObserver},
};

use crate::{
    command::IssuerCommand,
    multiprovider::MultiProvider,
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

    pub async fn set_solver_weights(
        &mut self,
        index: Arc<IndexInstance>,
        basket: Arc<Basket>,
        price: Amount,
        observer: OneShotSingleObserver<Amount>,
    ) -> eyre::Result<()> {
        let (provider, _rpc_url) = self
            .providers
            .next_provider()
            .current()
            .ok_or_eyre("No provider")?;

        let from_address = provider.default_signer_address();

        let collateral_token_converter =
            AmountConverter::new(index.get_collateral_token_precision());

        let price = collateral_token_converter.from_amount(price)?;
        let weights = bytes_from_weights(basket);

        let receipt = index
            .solver_weights_set_from(provider, from_address, &weights, price)
            .await?;

        let gas_amount = compute_gas_used(receipt)?;
        observer.one_shot_publish_single(gas_amount);
        Ok(())
    }

    pub async fn mint_index(
        &mut self,
        index: Arc<IndexInstance>,
        receipient: Address,
        amount: Amount,
        seq_num_execution_report: U256,
        observer: OneShotSingleObserver<Amount>,
    ) -> eyre::Result<()> {
        let account_name = self.account_name.clone();
        let index_token_converter = AmountConverter::new(index.get_index_token_precision());
        let amount = index_token_converter.from_amount(amount)?;

        let gas_amount = self
            .providers
            .try_execute(async move |provider, rpc_url| -> eyre::Result<Amount> {
                tracing::info!(
                        %account_name,
                        %rpc_url,
                        "Minting {} of Index for {}", amount, receipient);

                let from_address = provider.default_signer_address();
                let receipt = index
                    .mint_index_from(
                        provider,
                        from_address,
                        receipient,
                        amount,
                        seq_num_execution_report,
                    )
                    .await?;

                let tx = receipt.transaction_hash;
                let gas_amount = match compute_gas_used(receipt) {
                    Ok(gas) => gas,
                    Err(err) => {
                        tracing::warn!("⚠️ Failed to compute value gas used: {:?}", err);
                        Amount::ZERO
                    }
                };

                tracing::info!(
                        %account_name,
                        %rpc_url,
                        "💳 Minted Index for {} gas used {} tx: {}", receipient, gas_amount, tx);

                Ok(gas_amount)
            })
            .await?;

        observer.one_shot_publish_single(gas_amount);
        Ok(())
    }

    pub async fn send_issuer_command(
        &mut self,
        index: Arc<IndexInstance>,
        command: IssuerCommand,
    ) -> eyre::Result<()> {
        match command {
            IssuerCommand::SetSolverWeights {
                basket,
                price,
                observer,
            } => {
                self.set_solver_weights(index, basket, price, observer)
                    .await?;
            }
            IssuerCommand::MintIndex {
                receipient,
                amount,
                seq_num_execution_report,
                observer,
            } => {
                self.mint_index(
                    index,
                    receipient,
                    amount,
                    seq_num_execution_report,
                    observer,
                )
                .await?;
            }
            IssuerCommand::BurnIndex { .. } => {
                todo!("Burn Index is not implemented yet");
            }
            IssuerCommand::Withdraw { .. } => {
                todo!("Withdraw is not implemented yet");
            }
        }
        Ok(())
    }
}
