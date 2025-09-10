use core::num;
use std::time::Duration;

use alloy::providers::{DynProvider, Provider, WalletProvider};
use alloy_primitives::Address;

use otc_custody::{contracts::OTCCustody, custody_client::CustodyClient};
use symm_core::core::{bits::Amount, functional::OneShotPublishSingle};
use tokio::time::sleep;

use crate::{
    command::CustodyCommand,
    util::{amount_converter::AmountConverter, gas_util::compute_gas_used},
};

pub struct RpcCustodySession<P>
where
    P: Provider + WalletProvider + Clone + 'static,
{
    provider: P,
    account_name: String,
}

impl<P> RpcCustodySession<P>
where
    P: Provider + WalletProvider + Clone + 'static,
{
    pub fn new(account_name: String, provider: P) -> Self {
        Self {
            account_name,
            provider,
        }
    }

    pub async fn send_custody_command(
        &self,
        custody_client: CustodyClient,
        token_address: Address,
        command: CustodyCommand,
    ) -> eyre::Result<()> {
        let provider = &self.provider;
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
                    "Routing {} collateral from custody to {}",
                    amount,
                    destination
                );

                let amount = converter.from_amount(amount)?;
                let provider = DynProvider::new(provider.clone());

                let mut total_gas_amount = Amount::ZERO;
                let num_retries = 3;
                let backoff_seconds = 2;

                // TODO: Design better retry mechanism
                for i in 0..num_retries {
                    let receipt = match custody_client
                        .route_collateral_to_from(
                            &provider,
                            &from_address,
                            &destination,
                            &token_address,
                            amount,
                        )
                        .await
                    {
                        Ok(r) => r,
                        Err(e) => {
                            if i == num_retries - 1 {
                                Err(e)?
                            } else {
                                sleep(Duration::from_secs(backoff_seconds)).await;
                                tracing::warn!(account_name= %self.account_name, "⚠️ Retrying route collateral");
                                continue;
                            }
                        }
                    };

                    let tx = receipt.transaction_hash;
                    let gas_amount = compute_gas_used(receipt)?;

                    total_gas_amount += gas_amount;

                    tracing::info!(
                        account_name = %self.account_name,
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
