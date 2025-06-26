use std::sync::{Arc, RwLock as ComponentLock};

use super::config::ConfigBuildError;
use derive_builder::Builder;

use chrono::Utc;
use symm_core::core::{
    bits::{Address, Amount, ClientOrderId, Symbol},
    functional::{
        IntoObservableSingleVTable, NotificationHandlerOnce, PublishSingle, SingleObserver,
    },
};

use crate::collateral::collateral_router::{
    CollateralBridge, CollateralDesignation, CollateralRouter, CollateralRouterEvent,
};
use eyre::{eyre, OptionExt, Result};

struct SimpleDesignation {
    type_: Symbol,
    name: Symbol,
    collateral_symbol: Symbol,
    full_name: Symbol,
}

impl SimpleDesignation {
    fn new(full_name: &str) -> Self {
        let (type_, tail) = full_name.split_once(":").unwrap();
        let (name, collateral_symbol) = tail.split_once(":").unwrap();
        Self {
            type_: Symbol::from(type_),
            name: Symbol::from(name),
            collateral_symbol: Symbol::from(collateral_symbol),
            full_name: Symbol::from(full_name),
        }
    }
}

impl CollateralDesignation for SimpleDesignation {
    fn get_type(&self) -> Symbol {
        self.type_.clone()
    }

    fn get_name(&self) -> Symbol {
        self.name.clone()
    }

    fn get_collateral_symbol(&self) -> Symbol {
        self.collateral_symbol.clone()
    }

    fn get_full_name(&self) -> Symbol {
        self.full_name.clone()
    }

    fn get_balance(&self) -> Amount {
        Amount::ZERO
    }
}

struct SimpleBridge {
    observer: SingleObserver<CollateralRouterEvent>,
    source: Arc<ComponentLock<SimpleDesignation>>,
    destination: Arc<ComponentLock<SimpleDesignation>>,
}

impl SimpleBridge {
    fn new(source: &str, destination: &str) -> Self {
        Self {
            observer: SingleObserver::new(),
            source: Arc::new(ComponentLock::new(SimpleDesignation::new(source))),
            destination: Arc::new(ComponentLock::new(SimpleDesignation::new(destination))),
        }
    }
}

impl IntoObservableSingleVTable<CollateralRouterEvent> for SimpleBridge {
    fn set_observer(&mut self, observer: Box<dyn NotificationHandlerOnce<CollateralRouterEvent>>) {
        self.observer.set_observer(observer);
    }
}

impl CollateralBridge for SimpleBridge {
    fn get_source(&self) -> Arc<ComponentLock<dyn CollateralDesignation>> {
        self.source.clone()
    }

    fn get_destination(&self) -> Arc<ComponentLock<dyn CollateralDesignation>> {
        self.destination.clone()
    }

    fn transfer_funds(
        &self,
        chain_id: u32,
        address: Address,
        client_order_id: ClientOrderId,
        route_from: Symbol,
        route_to: Symbol,
        amount: Amount,
        cumulative_fee: Amount,
    ) -> Result<()> {
        self.observer
            .publish_single(CollateralRouterEvent::HopComplete {
                chain_id,
                address,
                client_order_id,
                timestamp: Utc::now(),
                source: self.source.read().unwrap().get_full_name(),
                destination: self.destination.read().unwrap().get_full_name(),
                route_from,
                route_to,
                amount,
                fee: cumulative_fee,
            });
        Ok(())
    }
}

fn build_collateral_router(
    collateral_router: Arc<ComponentLock<CollateralRouter>>,
    chain_id: u32,
    source: &str,
    destination: &str,
) -> Result<()> {
    if let Ok(mut router) = collateral_router.write() {
        router.add_bridge(Arc::new(ComponentLock::new(SimpleBridge::new(
            source,
            destination,
        ))));
        router.add_route(&[Symbol::from(source), Symbol::from(destination)])?;
        router.add_chain_source(chain_id, Symbol::from(source))?;
        router.set_default_destination(Symbol::from(destination))?;
        Ok(())
    } else {
        Err(eyre!("Failed to obtain lock on collateral router"))
    }
}

#[derive(Clone, Builder)]
#[builder(
    pattern = "owned",
    build_fn(name = "try_build", error = "ConfigBuildError")
)]
pub struct CollateralRouterConfig {
    #[builder(setter(skip))]
    router: Option<Arc<ComponentLock<CollateralRouter>>>,
}

impl CollateralRouterConfig {
    #[must_use]
    pub fn builder() -> CollateralRouterConfigBuilder {
        CollateralRouterConfigBuilder::default()
    }

    pub fn expect_router_cloned(&self) -> Arc<ComponentLock<CollateralRouter>> {
        self.router.clone().ok_or(()).expect("Failed to get router")
    }

    pub fn try_get_collateral_router_cloned(&self) -> Result<Arc<ComponentLock<CollateralRouter>>> {
        self.router
            .clone()
            .ok_or_eyre("Failed to get collateral router")
    }
}

impl CollateralRouterConfigBuilder {
    pub fn build(self) -> Result<CollateralRouterConfig, ConfigBuildError> {
        let mut config = self.try_build()?;

        config
            .router
            .replace(Arc::new(ComponentLock::new(CollateralRouter::new())));

        Ok(config)
    }
}

#[derive(Clone, Builder)]
#[builder(
    pattern = "owned",
    build_fn(name = "try_build", error = "ConfigBuildError")
)]
pub struct SimpleCollateralRouterConfig {
    #[builder(setter(into, strip_option), default)]
    chain_id: u32,

    #[builder(setter(into, strip_option), default)]
    source: String,

    #[builder(setter(into, strip_option), default)]
    destination: String,
}

impl SimpleCollateralRouterConfig {
    #[must_use]
    pub fn builder() -> SimpleCollateralRouterConfigBuilder {
        SimpleCollateralRouterConfigBuilder::default()
    }
}

impl SimpleCollateralRouterConfigBuilder {
    pub fn build(self) -> Result<CollateralRouterConfig, ConfigBuildError> {
        let simple_config = self.try_build()?;
        let config = CollateralRouterConfig::builder().build()?;

        let collateral_router = config.try_get_collateral_router_cloned()?;

        build_collateral_router(
            collateral_router,
            simple_config.chain_id,
            simple_config.source.as_str(),
            simple_config.destination.as_str(),
        )?;

        Ok(config)
    }
}
