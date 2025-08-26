use crate::config::ConfigureBinanceAccess;
use eyre::Result;
use symm_core::order_sender::order_connector::SessionId;

pub struct Credentials {
    account_name: String,
    enable_trading: bool,
    get_api_key_fn: Box<dyn Fn() -> Option<String> + Send + Sync>,
    get_secret_fn: Box<dyn Fn() -> Option<String> + Send + Sync>,
    get_private_key_file_fn: Box<dyn Fn() -> Option<String> + Send + Sync>,
    get_private_key_passphrase_fn: Box<dyn Fn() -> Option<String> + Send + Sync>,
}

impl Credentials {
    pub fn new(
        account_name: String,
        enable_trading: bool,
        get_api_key_fn: impl Fn() -> Option<String> + Send + Sync + 'static,
        get_secret_fn: impl Fn() -> Option<String> + Send + Sync + 'static,
        get_private_key_file_fn: impl Fn() -> Option<String> + Send + Sync + 'static,
        get_private_key_passphrase_fn: impl Fn() -> Option<String> + Send + Sync + 'static,
    ) -> Self {
        Self {
            account_name,
            enable_trading,
            get_api_key_fn: Box::new(get_api_key_fn),
            get_secret_fn: Box::new(get_secret_fn),
            get_private_key_file_fn: Box::new(get_private_key_file_fn),
            get_private_key_passphrase_fn: Box::new(get_private_key_passphrase_fn),
        }
    }

    pub(crate) fn get_account_name(&self) -> String {
        self.account_name.clone()
    }

    pub fn account_name(&self) -> &str {
        &self.account_name
    }

    pub(crate) fn should_enable_trading(&self) -> bool {
        self.enable_trading
    }

    pub(crate) fn into_session_id(&self) -> SessionId {
        SessionId::from(self.get_account_name())
    }

    fn get_api_key(&self) -> Option<String> {
        (*self.get_api_key_fn)()
    }

    fn get_api_secret(&self) -> Option<String> {
        (*self.get_secret_fn)()
    }

    fn get_private_key_file(&self) -> Option<String> {
        (*self.get_private_key_file_fn)()
    }

    fn get_private_key_passphrase(&self) -> Option<String> {
        (*self.get_private_key_passphrase_fn)()
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
