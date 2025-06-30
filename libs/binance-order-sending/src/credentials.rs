use crate::config::ConfigureBinanceAccess;
use eyre::Result;
use symm_core::order_sender::order_connector::SessionId;

pub struct Credentials {
    api_key: String,
    enable_trading: bool,
    get_secret_fn: Box<dyn Fn() -> Option<String> + Send + Sync>,
    get_private_key_file_fn: Box<dyn Fn() -> Option<String> + Send + Sync>,
    get_private_key_passphrase_fn: Box<dyn Fn() -> Option<String> + Send + Sync>,
}

impl Credentials {
    pub fn new(
        api_key: String,
        enable_trading: bool,
        get_secret_fn: impl Fn() -> Option<String> + Send + Sync + 'static,
        get_private_key_file_fn: impl Fn() -> Option<String> + Send + Sync + 'static,
        get_private_key_passphrase_fn: impl Fn() -> Option<String> + Send + Sync + 'static,
    ) -> Self {
        Self {
            api_key,
            enable_trading,
            get_secret_fn: Box::new(get_secret_fn),
            get_private_key_file_fn: Box::new(get_private_key_file_fn),
            get_private_key_passphrase_fn: Box::new(get_private_key_passphrase_fn),
        }
    }

    pub fn get_api_key(&self) -> String {
        self.api_key.clone()
    }

    pub (crate) fn should_enable_trading(&self) -> bool {
        self.enable_trading
    }

    pub(crate) fn get_api_secret(&self) -> Option<String> {
        (*self.get_secret_fn)()
    }

    pub(crate) fn get_private_key_file(&self) -> Option<String> {
        (*self.get_private_key_file_fn)()
    }

    pub(crate) fn get_private_key_passphrase(&self) -> Option<String> {
        (*self.get_private_key_passphrase_fn)()
    }

    pub(crate) fn into_session_id(&self) -> SessionId {
        SessionId::from(self.get_api_key())
    }
}

pub trait ConfigureBinanceUsingCredentials
where
    Self: Sized,
{
    fn configure(self, credentials: &Credentials) -> Result<Self>;
}

impl<T> ConfigureBinanceUsingCredentials for T
where
    T: ConfigureBinanceAccess,
{
    fn configure(self, credentials: &Credentials) -> Result<Self> {
        self.configure(
            credentials.get_api_key(),
            credentials.get_api_secret(),
            credentials.get_private_key_file(),
            credentials.get_private_key_passphrase(),
        )
    }
}
