use std::sync::{Arc, RwLock as ComponentLock};

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
use eyre::Result;

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

pub fn build_collateral_router(
    chain_id: u32,
    source: &str,
    destination: &str,
) -> Arc<ComponentLock<CollateralRouter>> {
    let collateral_router = Arc::new(ComponentLock::new(CollateralRouter::new()));

    if let Ok(mut router) = collateral_router.write() {
        router
            .add_bridge(Arc::new(ComponentLock::new(SimpleBridge::new(
                source,
                destination,
            ))))
            .unwrap();
        router
            .add_route(&[Symbol::from(source), Symbol::from(destination)])
            .unwrap();
        router
            .add_chain_source(chain_id, Symbol::from(source))
            .unwrap();
        router
            .set_default_destination(Symbol::from(destination))
            .unwrap();
    } else {
        unreachable!()
    }

    collateral_router
}
