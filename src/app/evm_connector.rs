use std::sync::{Arc, RwLock as ComponentLock};

use alloy::primitives;
use alloy_evm_connector::{designation::EvmCollateralDesignation, evm_connector::EvmConnector};
use async_trait::async_trait;
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
pub struct EvmConnectorConfig {
    #[builder(setter(into, strip_option), default)]
    pub with_router: Option<CollateralRouterConfig>,

    #[builder(setter(skip))]
    evm_connector: Option<Arc<ComponentLock<EvmConnector>>>,
}

impl EvmConnectorConfig {
    #[must_use]
    pub fn builder() -> EvmConnectorConfigBuilder {
        EvmConnectorConfigBuilder::default()
    }

    pub fn expect_chain_connector_cloned(&self) -> Arc<ComponentLock<EvmConnector>> {
        self.evm_connector
            .clone()
            .ok_or(())
            .expect("Failed to get simple chain connector")
    }

    pub fn try_get_chain_connector_cloned(&self) -> Result<Arc<ComponentLock<EvmConnector>>> {
        self.evm_connector
            .clone()
            .ok_or_eyre("Failed to get simple chain connector")
    }

    pub async fn start(&self) -> eyre::Result<()> {
        match &self.evm_connector {
            Some(evm_connector) => {
                evm_connector
                    .write()
                    .map_err(|e| eyre!("Failed to access EVM connector: {:?}", e))?
                    .start()?;

                // TODO: Figure out what to do with those configurations from env
                // ---
                // Let's use hard-coded Anvil values for a moment...

                let chain_id = 42161;
                let anvil_url = String::from("http://127.0.0.1:8545");
                let anvil_default_pk = String::from(
                    "0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80",
                );

                evm_connector
                    .write()
                    .map_err(|e| eyre!("Failed to access EVM connector: {:?}", e))?
                    .connect_chain(chain_id, anvil_url, anvil_default_pk)
                    .await?;

                Ok(())
            }
            None => Err(eyre!("EVM connector not configures")),
        }
    }

    pub async fn stop(&self) -> eyre::Result<()> {
        match &self.evm_connector {
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

impl ChainConnectorConfig for EvmConnectorConfig {
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

impl EvmConnectorConfigBuilder {
    pub fn build_arc(self) -> Result<Arc<EvmConnectorConfig>, ConfigBuildError> {
        let mut config = self.try_build()?;

        let evm_connector = Arc::new(ComponentLock::new(EvmConnector::new()));
        config.evm_connector.replace(evm_connector.clone());

        if let Some(ref router_config) = config.with_router {
            let router = router_config.try_get_collateral_router_cloned()?;

            // TODO: Perhaps use JSON config with routes defined in it
            // ---
            // Let's use hard-coded Anvil values for a moment...

            let chain_id = 42161;

            let src_custody = Arc::new(std::sync::RwLock::new(
                EvmCollateralDesignation::arbitrum_usdc_with_name(
                    address!("0x70997970C51812dc3A010C7d01b50e0d17dc79C8"),
                    "CUSTODY_A",
                ),
            ));

            let dst_custody = Arc::new(std::sync::RwLock::new(
                EvmCollateralDesignation::arbitrum_usdc_with_name(
                    address!("0x3C44CdDdB6a900fa2b585dd299e03d12FA4293BC"),
                    "CUSTODY_B",
                ),
            ));

            let src_custody_name = src_custody
                .read()
                .map_err(|e| eyre!("Failed to access designation: {:?}", e))?
                .get_full_name();

            let dst_custody_name = dst_custody
                .read()
                .map_err(|e| eyre!("Failed to access designation: {:?}", e))?
                .get_full_name();

            let bridge = evm_connector
                .write()
                .map_err(|e| eyre!("Failed to access EVM connector: {:?}", e))?
                .create_bridge(src_custody.clone(), dst_custody.clone());

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
