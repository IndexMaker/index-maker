use binance_sdk::config::{
    ConfigurationRestApiBuilder, ConfigurationWebsocketApiBuilder, PrivateKey,
};
use eyre::{eyre, OptionExt, Result};

pub trait ConfigureBinanceAccess
where
    Self: Sized,
{
    fn configure(
        self,
        api_key: Option<String>,
        api_secret: Option<String>,
        private_key_file: Option<String>,
        private_key_passphrase: Option<String>,
    ) -> Result<Self>;
}

impl ConfigureBinanceAccess for ConfigurationRestApiBuilder {
    fn configure(
        self,
        api_key: Option<String>,
        api_secret: Option<String>,
        private_key_file: Option<String>,
        private_key_passphrase: Option<String>,
    ) -> Result<Self> {
        if let Some(api_key) = api_key {
            let builder = self.api_key(api_key);
            Ok(if let Some(private_key_file) = private_key_file {
                let private_key = PrivateKey::File(private_key_file);
                let builder = builder.private_key(private_key);
                if let Some(private_key_passphrase) = private_key_passphrase {
                    builder.private_key_passphrase(private_key_passphrase)
                } else {
                    builder
                }
            } else {
                builder.api_secret(
                    api_secret.ok_or_eyre("No API secret nor private key file specified")?,
                )
            })
        } else {
            Err(eyre!("No API key specified"))
        }
    }
}

impl ConfigureBinanceAccess for ConfigurationWebsocketApiBuilder {
    fn configure(
        self,
        api_key: Option<String>,
        api_secret: Option<String>,
        private_key_file: Option<String>,
        private_key_passphrase: Option<String>,
    ) -> Result<Self> {
        if let Some(api_key) = api_key {
            let builder = self.api_key(api_key);
            Ok(if let Some(private_key_file) = private_key_file {
                let private_key = PrivateKey::File(private_key_file);
                let builder = builder.private_key(private_key);
                if let Some(private_key_passphrase) = private_key_passphrase {
                    builder.private_key_passphrase(private_key_passphrase)
                } else {
                    builder
                }
            } else {
                builder.api_secret(
                    api_secret.ok_or_eyre("No API secret nor private key file specified")?,
                )
            })
        } else {
            Err(eyre!("No API key specified"))
        }
    }
}
