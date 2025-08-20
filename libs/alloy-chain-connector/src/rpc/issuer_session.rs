use alloy::providers::{Provider, WalletProvider};

use crate::{command::IssuerCommand, contracts::OTCIndex};

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

    pub async fn send_issuer_command(&self, command: IssuerCommand) -> eyre::Result<()> {
        let signer_address = self.provider.default_signer_address();
        let provider = &self.provider;
        let index = OTCIndex::new(signer_address, provider);

        match command {
            IssuerCommand::SetSolverWeights {
                timestamp,
                weights,
                price,
            } => {
                index.solverUpdate(timestamp, weights, price).call().await?;
            }
            IssuerCommand::MintIndex {
                target,
                amount,
                seq_num_execution_report,
            } => {
                index
                    .mint(target, amount, seq_num_execution_report)
                    .call()
                    .await?;
            }
            IssuerCommand::BurnIndex {
                amount,
                target,
                seq_num_new_order_single,
            } => {
                index
                    .burn(amount, target, seq_num_new_order_single)
                    .call()
                    .await?;
            }
            IssuerCommand::Withdraw {
                amount,
                to,
                verification_data,
                execution_report,
            } => {
                index
                    .withdraw(amount, to, verification_data, execution_report)
                    .call()
                    .await?;
            }
        }
        Ok(())
    }
}
