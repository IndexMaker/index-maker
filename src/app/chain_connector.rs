use std::sync::{Arc, RwLock as ComponentLock};

use alloy::primitives;
use alloy_chain_connector::{
    chain_connector::RealChainConnector,
    collateral::{
        signer_to_wallet_bridge::SignerWalletToWalletCollateralBridge,
        signer_wallet_designation::SignerWalletCollateralDesignation,
        wallet_designation::WalletCollateralDesignation,
    },
    credentials::Credentials,
};
use alloy_primitives::Address;
use eyre::{eyre, OptionExt, Result};
use primitives::address;
use symm_core::core::bits::Symbol;

use super::config::ConfigBuildError;
use derive_builder::Builder;

use index_core::{
    blockchain::chain_connector::ChainConnector,
    collateral::collateral_router::CollateralDesignation,
};

use crate::app::{collateral_router::CollateralRouterConfig, solver::ChainConnectorConfig};

#[derive(Clone, Builder)]
#[builder(
    pattern = "owned",
    build_fn(name = "try_build", error = "ConfigBuildError")
)]
pub struct RealChainConnectorConfig {
    #[builder(setter(into, strip_option), default)]
    pub with_router: Option<CollateralRouterConfig>,

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
                chain_connector
                    .write()
                    .map_err(|e| eyre!("Failed to access EVM connector: {:?}", e))?
                    .start()?;

                // TODO: Configure this
                // ---
                // Let's use hard-coded Anvil values for a moment...

                let chain_id = 42161;
                let anvil_url = String::from("http://127.0.0.1:8545");
                let usdc_address = address!("0xaf88d065e77c8cC2239327C5EDb3A432268e5831");

                // Some custody that will receive deposits and we'll route from
                // that to some other wallet designation.
                let custody1 = Credentials::new(
                    String::from("AnvilCustody1"),
                    chain_id,
                    usdc_address,
                    anvil_url,
                    Arc::new(|| {
                        String::from(
                            "0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80",
                        )
                    }),
                );

                // We need to logon as the owner of the custody. This will
                // create RPC session containing private key signer.
                chain_connector
                    .write()
                    .map_err(|e| eyre!("Failed to access EVM connector: {:?}", e))?
                    .logon([custody1])?;

                // We need to map chain ID to specific OTCIndex contract, so
                // that issuer commands will be routed to that contract.
                // ---
                // Temporarily put here no-address, as we need to have OTCIndex deployed.
                chain_connector
                    .write()
                    .map_err(|e| eyre!("Failed to access EVM connector: {:?}", e))?
                    .set_issuer(
                        chain_id,
                        String::from("AnvilIssuer1"),
                        Address::default(),
                    )?;

                Ok(())
            }
            None => Err(eyre!("EVM connector not configures")),
        }
    }

    pub async fn stop(&self) -> eyre::Result<()> {
        match &self.chain_connector {
            Some(evm_connector) => {
                evm_connector
                    .write()
                    .map_err(|e| eyre!("Failed to access EVM connector: {:?}", e))?
                    .stop()
                    .await?;

                Ok(())
            }
            None => Err(eyre!("EVM connector not configures")),
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
    pub fn build_arc(self) -> Result<Arc<RealChainConnectorConfig>, ConfigBuildError> {
        let mut config = self.try_build()?;

        let chain_connector = Arc::new(ComponentLock::new(RealChainConnector::new()));
        config.chain_connector.replace(chain_connector.clone());

        if let Some(ref router_config) = config.with_router {
            let router = router_config.try_get_collateral_router_cloned()?;

            // TODO: Perhaps use JSON config with routes defined in it
            // ---
            // Let's use hard-coded Anvil values for a moment...

            let chain_id = 42161;
            let usdc_symbol = Symbol::from("USDC");
            let usdc_address = address!("0xaf88d065e77c8cC2239327C5EDb3A432268e5831");

            let src_custody = Arc::new(std::sync::RwLock::new(
                SignerWalletCollateralDesignation::new(
                    Symbol::from("Anvil"),
                    Symbol::from("Source"),
                    Arc::downgrade(&chain_connector),
                    String::from("AnvilCustody1"),
                    usdc_symbol.clone(),
                    chain_id,
                    address!("0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266"),
                    usdc_address,
                ),
            ));

            let dst_custody = Arc::new(ComponentLock::new(WalletCollateralDesignation::new(
                Symbol::from("Anvil"),
                Symbol::from("Destination"),
                usdc_symbol,
                chain_id,
                address!("0x70997970C51812dc3A010C7d01b50e0d17dc79C8"),
                usdc_address,
            )));

            let src_custody_name = src_custody
                .read()
                .map_err(|e| eyre!("Failed to access designation: {:?}", e))?
                .get_full_name();

            let dst_custody_name = dst_custody
                .read()
                .map_err(|e| eyre!("Failed to access designation: {:?}", e))?
                .get_full_name();

            let bridge = Arc::new(ComponentLock::new(
                SignerWalletToWalletCollateralBridge::new(src_custody, dst_custody),
            ));

            let mut router_write = router
                .write()
                .map_err(|e| eyre!("Failed to access router: {:?}", e))?;

            // Add bridges
            router_write.add_bridge(bridge)?;

            // Configure possible routes
            router_write.add_route(&[src_custody_name.clone(), dst_custody_name.clone()])?;

            // Map incoming chain to source custody
            router_write.add_chain_source(chain_id, src_custody_name)?;

            // Tell final destination custody
            router_write.set_default_destination(dst_custody_name)?;
        }

        Ok(Arc::new(config))
    }
}
