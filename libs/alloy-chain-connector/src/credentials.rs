use std::{collections::VecDeque, sync::Arc};

use alloy::{
    providers::{Provider, ProviderBuilder, WalletProvider},
    signers::local::PrivateKeySigner,
};
use chrono::{DateTime, Utc};
use eyre::eyre;
use futures::future::join_all;
use itertools::Itertools;
use parking_lot::Mutex;
use symm_core::core::bits::Address;
use tokio::sync::mpsc::{unbounded_channel, Receiver, Sender};

pub struct SharedSessionDataInner {
    pub last_block_number: u64,
    pub update_timestamp: DateTime<Utc>,
}

impl SharedSessionDataInner {
    pub fn new() -> Self {
        Self {
            last_block_number: 0,
            update_timestamp: DateTime::default(),
        }
    }
}

pub struct SharedSessionData {
    pub chain_id: u32,
    pub rpc_urls: Vec<String>,
    pub poll_interval: std::time::Duration,
    pub poll_backoff_period: std::time::Duration,
    pub retry_backoff: std::time::Duration,
    pub max_poll_failures: usize,
    pub max_retries: usize,
    pub inner: Mutex<SharedSessionDataInner>,
}

impl SharedSessionData {
    pub fn new(chain_id: u32, rpc_urls: impl IntoIterator<Item = String>) -> Self {
        Self {
            chain_id,
            rpc_urls: rpc_urls.into_iter().collect(),
            // TOOD: Configure me
            poll_interval: std::time::Duration::from_secs(10),
            poll_backoff_period: std::time::Duration::from_secs(10),
            retry_backoff: std::time::Duration::from_secs(2),
            max_poll_failures: 10,
            max_retries: 3,
            inner: Mutex::new(SharedSessionDataInner::new()),
        }
    }
}

#[derive(Clone)]
pub struct MultiProvider<T>
where
    T: Provider + Clone + 'static,
{
    shared_data: Arc<SharedSessionData>,
    providers: VecDeque<(T, String)>,
}

impl<T> MultiProvider<T>
where
    T: Provider + Clone + 'static,
{
    pub fn new(
        shared_data: Arc<SharedSessionData>,
        providers: impl IntoIterator<Item = (T, String)>,
    ) -> Self {
        Self {
            shared_data,
            providers: providers.into_iter().collect(),
        }
    }

    pub fn get_shared_data(&self) -> &Arc<SharedSessionData> {
        &self.shared_data
    }

    pub fn with_shared_date<R>(&self, cb: impl Fn(&SharedSessionData) -> R) -> R {
        cb(&self.shared_data)
    }

    pub async fn next_provider(&mut self) -> &Self {
        self.providers.rotate_left(1);
        self
    }

    pub fn current(&self) -> Option<&(T, String)> {
        self.providers.front()
    }
    
    pub async fn next_n_providers(&mut self, n: usize) -> &Self {
        self.providers.rotate_left(n);
        self
    }

    pub fn current_n(&self, n: usize) -> Vec<&(T, String)> {
        self.providers.iter().take(n).collect_vec()
    }
}

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
}
