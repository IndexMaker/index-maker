use alloy::providers::{DynProvider, Provider, WalletProvider};
use alloy_primitives::Address;

use alloy_rpc_types_eth::TransactionReceipt;
use eyre::OptionExt;
use otc_custody::{contracts::OTCCustody, custody_client::CustodyClient};
use symm_core::core::{
    bits::Amount,
    functional::{OneShotPublishSingle, OneShotSingleObserver},
};
use tokio::time::sleep;

use crate::{
    command::CustodyCommand,
    multiprovider::MultiProvider,
    util::{amount_converter::AmountConverter, gas_util::compute_gas_used},
};

pub struct RpcCustodySession<P>
where
    P: Provider + WalletProvider + Clone + 'static,
{
    providers: MultiProvider<P>,
    account_name: String,
}

impl<P> RpcCustodySession<P>
where
    P: Provider + WalletProvider + Clone + 'static,
{
    pub fn new(account_name: String, providers: MultiProvider<P>) -> Self {
        Self {
            account_name,
            providers,
        }
    }

    pub async fn custody_to_address(
        &mut self,
        custody_client: CustodyClient,
        token_address: Address,
        destination: Address,
        amount: Amount,
        observer: OneShotSingleObserver<Amount>,
    ) -> eyre::Result<()> {
        let account_name = self.account_name.clone();
        let decimals = custody_client.get_collateral_token_precision();
        let converter = AmountConverter::new(decimals);
        let amount = converter.from_amount(amount)?;

        let gas_amount = self
            .providers
            .try_execute(async move |provider, rpc_url| -> eyre::Result<Amount> {
                let from_address = provider.default_signer_address();

                tracing::info!(
                    %account_name,
                    %rpc_url,
                    "Routing {} collateral from custody to {}",
                    amount,
                    destination
                );

                let dyn_provider = DynProvider::new(provider.clone());

                let balance = custody_client
                    .get_collateral_token_balance(&dyn_provider)
                    .await?;

                if balance < amount {
                    tracing::warn!(
                                %account_name,
                                %rpc_url,
                                %balance,
                                %amount,
                                "❗️ Insufficient collateral balance in custody");

                    return Err(eyre::eyre!("Insufficient collateral balance in custody"));
                } else {
                    tracing::info!(
                                %account_name,
                                %rpc_url,
                                %balance,
                                %amount,
                                "Collateral balance in custody");
                }

                let receipt = custody_client
                    .route_collateral_to_from(
                        &dyn_provider,
                        &from_address,
                        &destination,
                        &token_address,
                        amount,
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
                    "🏦 Collateral routed to {} gas used {} tx {}",
                    destination,
                    gas_amount,
                    tx
                );

                Ok(gas_amount)
            })
            .await?;

        observer.one_shot_publish_single(gas_amount);

        Ok(())
    }

    pub async fn address_to_custody(
        &mut self,
        custody_client: CustodyClient,
        token_address: Address,
        amount: Amount,
        observer: OneShotSingleObserver<Amount>,
    ) -> eyre::Result<()> {
        let (provider, _rpc_url) = self
            .providers
            .next_provider()
            .current()
            .ok_or_eyre("No provider")?;

        let from_address = provider.default_signer_address();
        let decimals = custody_client.get_collateral_token_precision();
        let converter = AmountConverter::new(decimals);
        let amount = converter.from_amount(amount)?;

        let custody_id = custody_client.get_custody_id();
        let custody_address = custody_client.get_custody_address();
        let custody = OTCCustody::new(*custody_address, provider);

        let receipt = custody
            .addressToCustody(custody_id, token_address, amount)
            .from(from_address)
            .send()
            .await?
            .get_receipt()
            .await?;

        let gas_amount = compute_gas_used(receipt)?;
        observer.one_shot_publish_single(gas_amount);
        Ok(())
    }

    pub async fn get_custody_balances(
        &mut self,
        custody_client: CustodyClient,
        token_address: Address,
        observer: OneShotSingleObserver<Amount>,
    ) -> eyre::Result<()> {
        let (provider, _rpc_url) = self
            .providers
            .next_provider()
            .current()
            .ok_or_eyre("No provider")?;

        let from_address = provider.default_signer_address();
        let decimals = custody_client.get_collateral_token_precision();
        let converter = AmountConverter::new(decimals);

        let custody_id = custody_client.get_custody_id();
        let custody_address = custody_client.get_custody_address();
        let custody = OTCCustody::new(*custody_address, provider);

        let balance = custody
            .getCustodyBalances(custody_id, token_address)
            .from(from_address)
            .call()
            .await?;

        let balance = converter.into_amount(balance)?;
        observer.one_shot_publish_single(balance);
        Ok(())
    }

    pub async fn send_custody_command(
        &mut self,
        custody_client: CustodyClient,
        token_address: Address,
        command: CustodyCommand,
    ) -> eyre::Result<()> {
        match command {
            CustodyCommand::CustodyToAddress {
                destination,
                amount,
                observer,
            } => {
                self.custody_to_address(
                    custody_client,
                    token_address,
                    destination,
                    amount,
                    observer,
                )
                .await?;
            }
            CustodyCommand::AddressToCustody { amount, observer } => {
                self.address_to_custody(custody_client, token_address, amount, observer)
                    .await?;
            }
            CustodyCommand::GetCustodyBalances { observer } => {
                self.get_custody_balances(custody_client, token_address, observer)
                    .await?;
            }
        }

        Ok(())
    }
}
