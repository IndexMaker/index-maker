use std::sync::{Arc, RwLock as ComponentLock};

use crate::app::collateral_router::CollateralRouterConfig;

use super::config::ConfigBuildError;
use derive_builder::Builder;

use chrono::Utc;
use rust_decimal::dec;
use safe_math::safe;
use symm_core::core::{
    bits::{Address, Amount, ClientOrderId, Symbol},
    decimal_ext::DecimalExt,
    functional::{
        IntoObservableSingleVTable, NotificationHandlerOnce, PublishSingle, SingleObserver,
    },
};

use eyre::{OptionExt, Result};
use index_core::collateral::{self, collateral_router::{
    self, CollateralBridge, CollateralDesignation, CollateralRouterEvent, CollateralRoutingStatus
}};

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

    fn new_with_symbol(full_name: &str, symbol: &str) -> Self {
        let (type_, tail) = full_name.split_once(":").unwrap();
        let (name, collateral_symbol) = tail.split_once(":").unwrap();
        let name = format!("{}_{}", name, symbol);
        let full_name = format!("{}:{}:{}", type_, name, collateral_symbol);
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
    fn new(source: &str, symbol: &str, destination: &str) -> Self {
        Self {
            observer: SingleObserver::new(),
            source: Arc::new(ComponentLock::new(SimpleDesignation::new_with_symbol(
                source, symbol,
            ))),
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
        let fee_rate = dec!(0.001);
        let fee = safe!(amount * fee_rate).ok_or_eyre("Math problem")?;
        let amount = safe!(amount - fee).ok_or_eyre("Math problem")?;
        let cumulative_fee = safe!(cumulative_fee + fee).ok_or_eyre("Math problem")?;
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
                status: CollateralRoutingStatus::Success
            });
        Ok(())
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
    pub index_symbols: Vec<Symbol>,

    #[builder(setter(into, strip_option), default)]
    source: String,

    #[builder(setter(into, strip_option), default)]
    destination: String,

    #[builder(setter(into, strip_option), default)]
    pub with_router: Option<CollateralRouterConfig>,
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
        let config = simple_config
            .with_router
            .unwrap_or(CollateralRouterConfig::builder().build()?);

        let collateral_router = config.try_get_collateral_router_cloned()?;

        if let Ok(mut router) = collateral_router.write() {
            let chain_id = simple_config.chain_id;
            let destination = simple_config.destination;

            for symbol in simple_config.index_symbols {
                let bridge = Arc::new(ComponentLock::new(SimpleBridge::new(
                    simple_config.source.as_str(),
                    &symbol,
                    &destination,
                )));

                // should never panic.
                let source = bridge
                    .read()
                    .unwrap()
                    .get_source()
                    .read()
                    .unwrap()
                    .get_full_name()
                    .to_string();

                router.add_bridge(bridge)?;
                router.add_chain_source(chain_id, symbol, Symbol::from(source.as_str()))?;
                router.add_route(&[
                    Symbol::from(source.as_str()),
                    Symbol::from(destination.as_str()),
                ])?;
            }

            router.set_default_destination(Symbol::from(destination))?;
        } else {
            Err(ConfigBuildError::Other(String::from(
                "Failed to obtain lock on collateral router",
            )))?;
        }

        Ok(config)
    }
}
