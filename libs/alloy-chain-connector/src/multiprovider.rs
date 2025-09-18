use std::{collections::VecDeque, fmt::Debug, sync::Arc};

use alloy::providers::Provider;
use eyre::OptionExt;
use itertools::Itertools;

pub struct SharedSessionData {
    pub chain_id: u32,
    pub rpc_urls: Vec<String>,
    pub poll_interval: std::time::Duration,
    pub poll_backoff_period: std::time::Duration,
    pub retry_backoff: Vec<std::time::Duration>,
    pub max_poll_failures: usize,
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

    pub fn with_shared_data<R>(&self, cb: impl Fn(&SharedSessionData) -> R) -> R {
        cb(&self.shared_data)
    }

    pub fn is_empty(&self) -> bool {
        self.providers.is_empty()
    }

    pub fn len(&self) -> usize {
        self.providers.len()
    }

    pub fn next_provider(&mut self) -> &Self {
        self.providers.rotate_left(1);
        self
    }

    pub fn current(&self) -> Option<&(T, String)> {
        self.providers.front()
    }

    pub fn next_n_providers(&mut self, n: usize) -> &Self {
        self.providers.rotate_left(n);
        self
    }

    pub fn current_n(&self, n: usize) -> Vec<&(T, String)> {
        self.providers.iter().take(n).collect_vec()
    }

    pub fn get_providers(&self) -> &VecDeque<(T, String)> {
        &self.providers
    }

    pub async fn try_execute<F, R, E>(&mut self, mut cb: F) -> eyre::Result<R>
    where
        F: AsyncFnMut(&T, &String) -> Result<R, E>,
        E: Debug,
    {
        let retry_backoff = self.get_shared_data().retry_backoff.clone();

        for backoff_period in retry_backoff {
            let (provider, rpc_url) = self
                .next_provider()
                .current()
                .ok_or_eyre("No connected RPC clients available")?;

            match cb(provider, rpc_url).await {
                Ok(res) => return Ok(res),
                Err(err) => {
                    tracing::warn!(%rpc_url, "♻️ Retrying - ⚠️ Attempt failed: {:?}", err);
                    tokio::time::sleep(backoff_period).await;
                }
            }
        }

        Err(eyre::eyre!("❗️ All attempts failed"))
    }
}
