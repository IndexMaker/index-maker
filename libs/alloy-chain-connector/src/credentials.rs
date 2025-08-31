use alloy::{
    providers::{Provider, ProviderBuilder, WalletProvider}, signers::local::PrivateKeySigner
};
use eyre::eyre;
use symm_core::core::bits::Address;

#[derive(Clone)]
pub struct Credentials {
    account_name: String,
    chain_id: u32,
    usdc_address: Address,
    rpc_url: String,
    get_private_key_fn: std::sync::Arc<dyn Fn() -> String + Send + Sync>,
}

impl Credentials {
    pub fn new(
        account_name: String,
        chain_id: u32,
        usdc_address: Address,
        rpc_url: String,
        get_private_key_fn: std::sync::Arc<dyn Fn() -> String + Send + Sync>,
    ) -> Self {
        Self {
            account_name,
            chain_id,
            usdc_address,
            rpc_url,
            get_private_key_fn,
        }
    }

    pub fn get_account_name(&self) -> String {
        self.account_name.clone()
    }

    pub fn get_chain_id(&self) -> u32 {
        self.chain_id
    }

    pub fn get_usdc_address(&self) -> Address {
        self.usdc_address
    }

    pub fn get_rpc_url(&self) -> &str {
        &self.rpc_url
    }

    fn get_signer(&self) -> eyre::Result<PrivateKeySigner> {
        let signer = (*self.get_private_key_fn)()
            .parse::<PrivateKeySigner>()
            .map_err(|err| eyre!("Failed to parse private key: {:?}", err))?;

        Ok(signer)
    }

    pub fn get_signer_address(&self) -> eyre::Result<Address> {
        Ok(self.get_signer()?.address())
    }

    pub async fn connect(&self) -> eyre::Result<impl Provider + WalletProvider + Clone> {
        let provider = ProviderBuilder::new()
            .wallet(self.get_signer()?)
            .connect(&self.rpc_url)
            .await
            .map_err(|err| eyre!("Failed to connect RPC: {:?}", err))?;

        Ok(provider)
    }
    
    pub async fn connect_public(&self) -> eyre::Result<impl Provider + Clone> {
        let provider = ProviderBuilder::new()
            .connect(&self.rpc_url)
            .await
            .map_err(|err| eyre!("Failed to connect RPC: {:?}", err))?;

        Ok(provider)
    }
}
