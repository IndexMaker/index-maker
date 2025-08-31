use std::sync::{Arc, RwLock as ComponentLock};

use alloy::primitives;
use alloy_chain_connector::{
    chain_connector::RealChainConnector,
    collateral::{
        otc_custody_designation::OTCCustodyCollateralDesignation,
        otc_custody_to_wallet_bridge::OTCCustodyToWalletCollateralBridge,
        signer_to_wallet_bridge::SignerWalletToWalletCollateralBridge,
        signer_wallet_designation::SignerWalletCollateralDesignation,
        wallet_designation::WalletCollateralDesignation,
    },
    credentials::Credentials,
};
use alloy_primitives::Address;
use binance_order_sending::credentials;
use eyre::{eyre, Context, OptionExt, Result};
use otc_custody::{
    custody_authority::CustodyAuthority,
    custody_client::{CustodyClient, CustodyClientMethods},
    index::{index_deployment::IndexDeployment, index_deployment_serde::IndexMakerData},
};
use parking_lot::lock_api::RwLock;
use primitives::address;
use symm_core::core::{bits::Symbol, json_file_async::read_from_json_file_async};

use super::config::ConfigBuildError;
use derive_builder::Builder;

use index_core::{
    blockchain::chain_connector::ChainConnector,
    collateral::collateral_router::CollateralDesignation,
};

use crate::app::{collateral_router::CollateralRouterConfig, solver::ChainConnectorConfig};

#[derive(Builder, Clone)]
#[builder(
    pattern = "owned",
    build_fn(name = "try_build", error = "ConfigBuildError")
)]
pub struct RealChainConnectorConfig {
    #[builder(setter(into, strip_option), default)]
    pub with_router: Option<CollateralRouterConfig>,

    #[builder(setter(into))]
    with_credentials: Option<Credentials>,

    #[builder(setter(into))]
    with_custody_authority: Option<CustodyAuthority>,

    #[builder(setter(into))]
    with_config_file: String,

    #[builder(setter(skip))]
    chain_connector: Option<Arc<ComponentLock<RealChainConnector>>>,
}

impl RealChainConnectorConfig {
    #[must_use]
    pub fn builder() -> RealChainConnectorConfigBuilder {
        RealChainConnectorConfigBuilder::default()
    }

    pub fn expect_chain_connector_cloned(&self) -> Arc<ComponentLock<RealChainConnector>> {
        self.chain_connector
            .clone()
            .ok_or(())
            .expect("Failed to get simple chain connector")
    }

    pub fn try_get_chain_connector_cloned(&self) -> Result<Arc<ComponentLock<RealChainConnector>>> {
        self.chain_connector
            .clone()
            .ok_or_eyre("Failed to get simple chain connector")
    }

    pub async fn start(&self) -> eyre::Result<()> {
        match &self.chain_connector {
            Some(chain_connector) => {
                let credentials = self
                    .with_credentials
                    .clone()
                    .ok_or_eyre("Credentials missing")?;

                chain_connector
                    .write()
                    .map_err(|e| eyre!("Failed to access chain connector: {:?}", e))?
                    .start()?;

                chain_connector
                    .write()
                    .map_err(|e| eyre!("Failed to access chain connector: {:?}", e))?
                    .logon([credentials])?;

                Ok(())
            }
            None => Err(eyre!("Chain connector not configured")),
        }
    }

    pub async fn stop(&self) -> eyre::Result<()> {
        match &self.chain_connector {
            Some(evm_connector) => {
                evm_connector
                    .write()
                    .map_err(|e| eyre!("Failed to access chain connector: {:?}", e))?
                    .stop()
                    .await?;

                Ok(())
            }
            None => Err(eyre!("Chain connector not configures")),
        }
    }
}

impl ChainConnectorConfig for RealChainConnectorConfig {
    fn expect_chain_connector_cloned(
        &self,
    ) -> Arc<ComponentLock<dyn ChainConnector + Send + Sync>> {
        self.expect_chain_connector_cloned()
    }

    fn try_get_chain_connector_cloned(
        &self,
    ) -> Result<Arc<ComponentLock<dyn ChainConnector + Send + Sync>>> {
        self.try_get_chain_connector_cloned()
            .map(|x| x as Arc<ComponentLock<dyn ChainConnector + Send + Sync>>)
    }
}

impl RealChainConnectorConfigBuilder {
    pub async fn build_arc(self) -> Result<Arc<RealChainConnectorConfig>, ConfigBuildError> {
        tracing::info!("Configuring chain connector...");

        let mut config = self.try_build()?;

        let credentials = config
            .with_credentials
            .as_ref()
            .ok_or_else(|| ConfigBuildError::UninitializedField("with_credentials"))?;

        let index_operator = config
            .with_custody_authority
            .take()
            .ok_or_else(|| ConfigBuildError::UninitializedField("with_custody_authority"))?;

        let chain_connector = Arc::new(ComponentLock::new(RealChainConnector::new()));
        config.chain_connector.replace(chain_connector.clone());

        // Configure mapping { chain_id => Credentials }
        // This will set default RPC session for each chain.
        for credentials in config.with_credentials.iter() {
            let chain_id = credentials.get_chain_id();
            let account_name = credentials.get_account_name();
            chain_connector
                .write()
                .map_err(|e| eyre!("Failed to access chain connector: {:?}", e))?
                .set_issuer(chain_id, account_name.clone())?;
        }

        tracing::info!("✅ Operator credentials loaded successfully");

        if let Some(ref router_config) = config.with_router {
            let router = router_config.try_get_collateral_router_cloned()?;

            let index_maker_data =
                read_from_json_file_async::<IndexMakerData>(&config.with_config_file).await?;

            tracing::info!("✅ Index Deployment configuration successfully loaded");

            let chain_id = index_maker_data.deployment_builder_data.chain_id;
            let custody_address = index_maker_data.deployment_builder_data.custody_address;
            let trade_route = index_maker_data.deployment_builder_data.trade_route;
            let usdc_symbol = Symbol::from("USDC");
            let usdc_address = credentials.get_usdc_address();
            let account_name = credentials.get_account_name();

            let trading_custody = Arc::new(ComponentLock::new(WalletCollateralDesignation::new(
                Symbol::from("TradeRoute"),
                Symbol::from(trade_route.to_string()),
                usdc_symbol.clone(),
                chain_id,
                trade_route,
                usdc_address,
            )));

            let trading_custody_name = trading_custody.read().unwrap().get_full_name();

            router
                .write()
                .unwrap()
                .set_default_destination(trading_custody_name.clone())?;

            tracing::info!("✅ Set default trading custody");

            for data in index_maker_data.indexes {
                let index_deployment = IndexDeployment::new_from_deploy_data_serde(
                    index_operator.clone(),
                    data.deployment_data,
                );

                let index = index_deployment.into_index_at(data.index_address);

                let index_symbol = Symbol::from(index.get_symbol());
                let index_address = *index.get_index_address();
                let custody_id = index.get_custody_id();

                tracing::info!(
                    "✅ Index deployment configuration loaded ok: {} deployed at {} for custody {} at {}",
                    index_symbol,
                    index_address,
                    custody_id,
                    custody_address
                );

                if index.get_collateral_token_address().ne(&usdc_address) {
                    Err(eyre!("Collateral token address mismatch"))?;
                }

                if index.get_custody_address().ne(&custody_address) {
                    Err(eyre!("Custody address mismatch"))?;
                }

                let index = Arc::new(index);
                let custody_client = CustodyClient::new(index.clone());

                chain_connector
                    .write()
                    .unwrap()
                    .add_custody_client(custody_client)
                    .context("Failed to add custody client")?;

                chain_connector.write().unwrap().add_index(index)?;

                tracing::info!("✅ Added Index to RPC handlers");

                let index_custody =
                    Arc::new(ComponentLock::new(OTCCustodyCollateralDesignation::new(
                        Symbol::from("OTCCustody"),
                        Symbol::from(index_address.to_string()),
                        usdc_symbol.clone(),
                        chain_connector.clone(),
                        account_name.clone(),
                        chain_id,
                        custody_address,
                        usdc_address,
                        custody_id,
                    )));

                let bridge_to_trading =
                    Arc::new(ComponentLock::new(OTCCustodyToWalletCollateralBridge::new(
                        index_custody.clone(),
                        trading_custody.clone(),
                    )));

                let mut router_write = router.write().unwrap();

                let index_custody_name = index_custody.read().unwrap().get_full_name();

                router_write.add_bridge(bridge_to_trading)?;

                router_write
                    .add_route(&[index_custody_name.clone(), trading_custody_name.clone()])?;

                router_write.add_chain_source(
                    chain_id,
                    Symbol::from(index_symbol),
                    index_custody_name,
                )?;

                tracing::info!("✅ Added collateral route from Index custody to trading custody");
            }
        } else {
            tracing::warn!("❗️ No router configuration")
        }

        Ok(Arc::new(config))
    }
}
