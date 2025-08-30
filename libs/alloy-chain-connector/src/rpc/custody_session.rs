use alloy::providers::{DynProvider, Provider, WalletProvider};
use alloy_primitives::Address;
use symm_core::core::functional::PublishSingle;

use otc_custody::{contracts::OTCCustody, custody_client::CustodyClient};

use crate::{
    command::CustodyCommand,
    util::{amount_converter::AmountConverter, gas_util::compute_gas_used},
};

pub struct RpcCustodySession<P>
where
    P: Provider + WalletProvider + Clone + 'static,
{
    provider: P,
}

impl<P> RpcCustodySession<P>
where
    P: Provider + WalletProvider + Clone + 'static,
{
    pub fn new(provider: P) -> Self {
        Self { provider }
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
                let amount = converter.from_amount(amount)?;
                let provider = DynProvider::new(provider.clone());

                let receipt = custody_client
                    .route_collateral_to_from(
                        &provider,
                        &from_address,
                        &destination,
                        &token_address,
                        amount,
                    )
                    .await?;

                let gas_amount = compute_gas_used(&converter, receipt)?;
                observer.publish_single(gas_amount);
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

                let gas_amount = compute_gas_used(&converter, receipt)?;
                observer.publish_single(gas_amount);
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
                observer.publish_single(balance);
            }
        }

        Ok(())
    }
}
