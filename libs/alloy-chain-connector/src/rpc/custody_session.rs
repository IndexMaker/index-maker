use std::time::Duration;

use alloy::providers::{DynProvider, Provider, WalletProvider};
use alloy_primitives::Address;

use eyre::OptionExt;
use otc_custody::{contracts::OTCCustody, custody_client::CustodyClient};
use symm_core::core::{bits::Amount, functional::OneShotPublishSingle};
use tokio::time::sleep;

use crate::{
    command::CustodyCommand,
    credentials::MultiProvider,
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

    pub async fn send_custody_command(
        &mut self,
        custody_client: CustodyClient,
        token_address: Address,
        command: CustodyCommand,
    ) -> eyre::Result<()> {
        let (num_retries, backoff_period) = self
            .providers
            .with_shared_date(|s| (s.max_retries, s.retry_backoff));

        let mut current = self
            .providers
            .next_provider()
            .await
            .current()
            .ok_or_eyre("No provider")?;

        let (mut provider, mut rpc_url) = (&current.0, &current.1);

        let from_address = provider.default_signer_address();
        let decimals = custody_client.get_collateral_token_precision();
        let converter = AmountConverter::new(decimals);

        match command {
            CustodyCommand::CustodyToAddress {
                destination,
                amount,
                observer,
            } => {
                tracing::info!(
                    account_name = %self.account_name,
                    %rpc_url,
                    "Routing {} collateral from custody to {}",
                    amount,
                    destination
                );

                let amount = converter.from_amount(amount)?;

                let mut total_gas_amount = Amount::ZERO;

                for i in 0..num_retries {
                    let dyn_provider = DynProvider::new(provider.clone());

                    let receipt = match custody_client
                        .route_collateral_to_from(
                            &dyn_provider,
                            &from_address,
                            &destination,
                            &token_address,
                            amount,
                        )
                        .await
                    {
                        Ok(r) => r,
                        Err(e) => {
                            current = self
                                .providers
                                .next_provider()
                                .await
                                .current()
                                .ok_or_eyre("No provider")?;

                            provider = &current.0;
                            rpc_url = &current.1;

                            if i == num_retries - 1 {
                                Err(e)?
                            } else {
                                sleep(backoff_period).await;
                                tracing::warn!(
                                    account_name= %self.account_name,
                                    %rpc_url,
                                    "⚠️ Retrying route collateral");
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
                        "🏦 Collateral routed to {} gas used {} tx {}",
                        destination,
                        total_gas_amount,
                        tx
                    );

                    observer.one_shot_publish_single(total_gas_amount);
                    break;
                }
            }
            CustodyCommand::AddressToCustody { amount, observer } => {
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
            }
            CustodyCommand::GetCustodyBalances { observer } => {
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
            }
        }

        Ok(())
    }
}
