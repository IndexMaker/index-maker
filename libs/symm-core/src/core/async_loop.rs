use std::future::Future;

use eyre::{eyre, OptionExt, Report, Result};
use itertools::Either;
use tokio::{
    spawn,
    task::{JoinError, JoinHandle},
};
use tokio_util::sync::CancellationToken;

pub struct AsyncTask<T> {
    join_handle: JoinHandle<T>,
    cancel_token: CancellationToken,
}

impl<T> AsyncTask<T> {
    pub fn new(join_handle: JoinHandle<T>, cancel_token: CancellationToken) -> Self {
        Self {
            join_handle,
            cancel_token,
        }
    }

    pub fn signal_stop(&self) {
        self.cancel_token.cancel();
    }

    pub async fn stop(self) -> Result<T, JoinError> {
        self.cancel_token.cancel();
        self.join_handle.await
    }

    pub fn has_stopped(&self) -> bool {
        self.join_handle.is_finished()
    }
}

pub struct AsyncLoop<T> {
    async_task: Option<AsyncTask<T>>,
}

impl<T> AsyncLoop<T>
where
    T: Send + 'static,
{
    pub fn new() -> Self {
        Self { async_task: None }
    }

    pub fn start<Fut>(&mut self, f: impl FnOnce(CancellationToken) -> Fut)
    where
        Fut: Future<Output = T> + Send + 'static,
    {
        self.start_with_cancel_token(CancellationToken::new(), f);
    }

    pub fn start_with_cancel_token<Fut>(
        &mut self,
        cancel_token: CancellationToken,
        f: impl FnOnce(CancellationToken) -> Fut,
    ) where
        Fut: Future<Output = T> + Send + 'static,
    {
        let cancel_token_cloned = cancel_token.clone();

        self.async_task
            .replace(AsyncTask::new(spawn(f(cancel_token_cloned)), cancel_token));
    }

    pub fn signal_stop(&self) {
        self.async_task.as_ref().inspect(|t| t.signal_stop());
    }

    pub fn has_stopped(&self) -> bool {
        self.async_task.as_ref().map_or(false, |x| x.has_stopped())
    }

    pub async fn stop(&mut self) -> Result<T, Either<JoinError, Report>> {
        if let Some(task) = self.async_task.take() {
            task.stop().await.map_err(|err| Either::Left(err))
        } else {
            Err(Either::Right(eyre!("AsyncLoop is not running")))
        }
    }
}
