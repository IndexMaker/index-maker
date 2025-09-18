use std::sync::Arc;

use alloy::{
    providers::{Provider, ProviderBuilder, WalletProvider, WsConnect},
    signers::local::PrivateKeySigner,
};
use eyre::eyre;
use futures::future::join_all;
use itertools::Itertools;
use symm_core::core::bits::Address;

use crate::multiprovider::{MultiProvider, SharedSessionData};

#[derive(Clone)]
pub struct Credentials {
    account_name: String,
    shared_data: Arc<SharedSessionData>,
    get_private_key_fn: std::sync::Arc<dyn Fn() -> String + Send + Sync>,
}

impl Credentials {
    pub fn new(
        account_name: String,
        shared_data: Arc<SharedSessionData>,
        get_private_key_fn: std::sync::Arc<dyn Fn() -> String + Send + Sync>,
    ) -> Self {
        Self {
            account_name,
            shared_data,
            get_private_key_fn,
        }
    }

    pub fn get_account_name(&self) -> String {
        self.account_name.clone()
    }

    pub fn get_chain_id(&self) -> u32 {
        self.shared_data.chain_id
    }

    pub fn get_shared_data(&self) -> &SharedSessionData {
        &self.shared_data
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

    pub async fn connect_any(
        &self,
    ) -> eyre::Result<MultiProvider<impl Provider + WalletProvider + Clone>> {
        let signer = self.get_signer()?;

        let providers = self.shared_data.rpc_urls.iter().map(|rpc_url| {
            ProviderBuilder::new()
                .wallet(signer.clone())
                .connect(rpc_url)
        });

        let (providers, errors): (Vec<_>, Vec<_>) = join_all(providers)
            .await
            .into_iter()
            .zip(self.shared_data.rpc_urls.iter())
            .map(|(x, y)| x.map(|x| (x, y.clone())))
            .partition_result();

        if providers.is_empty() {
            Err(eyre!(
                "Failed to connect RPC: {:?}",
                errors
                    .into_iter()
                    .map(|err| format!("{:?}", err))
                    .join("; ")
            ))?;
        }

        Ok(MultiProvider::new(self.shared_data.clone(), providers))
    }

    pub async fn connect_any_public(&self) -> eyre::Result<MultiProvider<impl Provider + Clone>> {
        let providers = self
            .shared_data
            .rpc_urls
            .iter()
            .map(|rpc_url| ProviderBuilder::new().connect(rpc_url));

        let (providers, errors): (Vec<_>, Vec<_>) = join_all(providers)
            .await
            .into_iter()
            .zip(self.shared_data.rpc_urls.iter())
            .map(|(x, y)| x.map(|x| (x, y.clone())))
            .partition_result();

        if providers.is_empty() {
            Err(eyre!(
                "Failed to connect RPC: {:?}",
                errors
                    .into_iter()
                    .map(|err| format!("{:?}", err))
                    .join("; ")
            ))?;
        }

        Ok(MultiProvider::new(self.shared_data.clone(), providers))
    }

    pub async fn connect_any_ws(&self) -> eyre::Result<MultiProvider<impl Provider + Clone>> {
        let signer = self.get_signer()?;

        let providers = self.shared_data.rpc_urls.iter().filter_map(|rpc_url| {
            if rpc_url.starts_with("wss://") {
                tracing::info!(%rpc_url, "WsConnect");
                let ws = WsConnect::new(rpc_url);
                Some(ProviderBuilder::new().wallet(signer.clone()).connect_ws(ws))
            } else {
                None
            }
        });

        let (providers, errors): (Vec<_>, Vec<_>) = join_all(providers)
            .await
            .into_iter()
            .zip(self.shared_data.rpc_urls.iter())
            .map(|(x, y)| x.map(|x| (x, y.clone())))
            .partition_result();

        if providers.is_empty() {
            Err(eyre!(
                "Failed to connect RPC: {:?}",
                errors
                    .into_iter()
                    .map(|err| format!("{:?}", err))
                    .join("; ")
            ))?;
        }

        Ok(MultiProvider::new(self.shared_data.clone(), providers))
    }
}
