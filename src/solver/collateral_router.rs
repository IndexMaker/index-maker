use std::{
    collections::{HashMap, VecDeque},
    sync::Arc,
};

use chrono::{DateTime, TimeDelta, Utc};
use itertools::Itertools;
use parking_lot::RwLock;

use eyre::{OptionExt, Result};
use safe_math::safe;

use crate::core::{
    bits::{Address, Amount, ClientOrderId, PaymentId, Symbol},
    decimal_ext::DecimalExt,
    functional::{IntoObservableSingle, PublishSingle, SingleObserver},
};

use super::solver::{CollateralManagement, SetSolverOrderStatus};

pub enum CollateralTransferEvent {
    TransferComplete {
        chain_id: u32,
        address: Address,
        client_order_id: ClientOrderId,
        transfer_from: Symbol,
        transfer_to: Symbol,
        amount: Amount,
        fee: Amount,
    },
}

pub enum CollateralRouterEvent {
    HopComplete {
        chain_id: u32,
        address: Address,
        client_order_id: ClientOrderId,
        source: Symbol,
        destination: Symbol,
        route_from: Symbol,
        route_to: Symbol,
        amount: Amount,
        fee: Amount,
    },
}

pub trait CollateralDesignation: Send + Sync {
    /// e.g. EVM, CEFFU, BINANCE
    fn get_type(&self) -> Symbol;

    /// e.g. ARBITRUM, BASE, CEFFU, 1, 2
    fn get_name(&self) -> Symbol;

    /// e.g. USDC, USDT
    fn get_collateral_symbol(&self) -> Symbol;

    /// e.g. EVM:ARBITRUM:USDC
    fn get_full_name(&self) -> Symbol;

    /// Tell balance of the collateral in this designation
    fn get_balance(&self) -> Amount;
}

pub trait CollateralBridge: Send + Sync {
    /// e.g. EVM:ARBITRUM:USDC
    fn get_source(&self) -> Arc<RwLock<dyn CollateralDesignation>>;

    /// e.g. BINANCE:1:USDT
    fn get_destination(&self) -> Arc<RwLock<dyn CollateralDesignation>>;

    /// Transfer funds from this designation to target designation
    ///
    /// - chain_id - Chain ID from which IndexOrder originated
    /// - address - User address from whom IndexOrder originated
    /// - client_order_id - An ID of the IndexOrder that user assigned to it
    /// - amount - An amount to be transferred from source to destination
    ///
    /// This may be direct bridge or composite of several bridges.
    ///
    /// Bridging Rules:
    /// ===============
    /// * `EVM:ARBITRUM:USDC` => `EVM:BASE:USDC`
    /// * `EVM:BASE:USDC` => `CEFFU:USDC`
    /// * `CEFFU:USDC` => `BINANCE:1:USDC`
    /// * `BINANCE:1:USDC` => `BINANCE:1:USDT`
    /// * `BINANCE:1:USDC` => `BINANCE:2:USDC`
    fn transfer_funds(
        &self,
        chain_id: u32,
        address: Address,
        client_order_id: ClientOrderId,
        route_from: Symbol,
        route_to: Symbol,
        amount: Amount,
    ) -> Result<()>;
}

pub struct CollateralRouter {
    observer: SingleObserver<CollateralTransferEvent>,
    bridges: HashMap<(Symbol, Symbol), Arc<RwLock<dyn CollateralBridge>>>,
    routes: Vec<Vec<Symbol>>,
    chain_sources: HashMap<u32, Symbol>,
    default_destination: Option<Symbol>,
}

impl CollateralRouter {
    pub fn new() -> Self {
        Self {
            observer: SingleObserver::new(),
            bridges: HashMap::new(),
            routes: Vec::new(),
            chain_sources: HashMap::new(),
            default_destination: None,
        }
    }

    pub fn add_bridge(&mut self, bridge: Arc<RwLock<dyn CollateralBridge>>) -> Result<()> {
        let key = (|| {
            let bridge = bridge.read();
            (
                bridge.get_source().read().get_full_name(),
                bridge.get_destination().read().get_full_name(),
            )
        })();
        self.bridges
            .insert(key, bridge)
            .is_none()
            .then_some(())
            .ok_or_eyre("Duplicate designation ID")
    }

    pub fn add_route(&mut self, route: &[Symbol]) -> Result<()> {
        self.routes.push(route.into());
        Ok(())
    }

    pub fn add_chain_source(
        &mut self,
        chain_id: u32,
        collateral_designation: Symbol,
    ) -> Result<()> {
        self.chain_sources
            .insert(chain_id, collateral_designation)
            .is_none()
            .then_some(())
            .ok_or_eyre("Duplicate chain ID")
    }

    pub fn set_default_destination(&mut self, collateral_designation: Symbol) -> Result<()> {
        self.default_destination
            .replace(collateral_designation)
            .is_none()
            .then_some(())
            .ok_or_eyre("Default destination already set")
    }

    pub fn transfer_collateral(
        &self,
        collateral_management: CollateralManagement,
        amount: Amount,
    ) -> Result<()> {
        //
        // TODO: Scan CollatearalManagement and tell:
        // - what are the final destinations for collateral (multiple destinations)
        //
        // Note that for now we have single "default" destination
        //

        let transfer_from = self
            .chain_sources
            .get(&collateral_management.chain_id)
            .ok_or_eyre("Failed to find source")?;

        let transfer_to = self
            .default_destination
            .clone()
            .ok_or_eyre("Failed to find destination")?;

        let first_hop = self.next_hop(transfer_from, transfer_from, &transfer_to)?;

        first_hop.write().transfer_funds(
            collateral_management.chain_id,
            collateral_management.address,
            collateral_management.client_order_id,
            transfer_from.clone(),
            transfer_to.clone(),
            amount,
        )
    }

    fn next_hop(
        &self,
        source: &Symbol,
        route_from: &Symbol,
        route_to: &Symbol,
    ) -> Result<&Arc<RwLock<dyn CollateralBridge>>> {
        let route = self
            .routes
            .iter()
            .find(|route| match (route.first(), route.last()) {
                (Some(first), Some(last)) => route_from.eq(first) && route_to.eq(last),
                _ => false,
            })
            .ok_or_eyre("Route not found")?;

        println!("Found route: {}", route.iter().join(", "));

        let next_hop_name = if source.eq(route_from) {
            (route[0].clone(), route[1].clone())
        } else {
            let (pos, first) = route
                .iter()
                .find_position(|hop| source.eq(*hop))
                .ok_or_eyre("Hop not found")?;
            let second = &route[pos + 1];
            (first.clone(), second.clone())
        };

        let next_hop = self
            .bridges
            .get(&next_hop_name)
            .ok_or_eyre("Cannot find next hop")?;

        Ok(next_hop)
    }

    pub fn handle_collateral_router_event(&mut self, event: CollateralRouterEvent) -> Result<()> {
        match event {
            CollateralRouterEvent::HopComplete {
                chain_id,
                address,
                client_order_id,
                source,
                destination,
                route_from,
                route_to,
                amount,
                fee,
            } => {
                if route_to.eq(&destination) {
                    println!(
                        "Route Complete for {} {} {}: {} => {} {:0.5} {:0.5}",
                        chain_id, address, client_order_id, route_from, route_to, amount, fee
                    );
                    self.observer
                        .publish_single(CollateralTransferEvent::TransferComplete {
                            chain_id,
                            address,
                            client_order_id,
                            transfer_from: route_from,
                            transfer_to: route_to,
                            amount,
                            fee,
                        });
                        Ok(())
                } else {
                    let next_hop = self.next_hop(&destination, &route_from, &route_to)?;
                    println!(
                        "Route Hop for {} {} {}: ({}) {} => {} => {} ({}) {:0.5} {:0.5}",
                        chain_id,
                        address,
                        client_order_id,
                        route_from,
                        source,
                        next_hop.read().get_source().read().get_full_name(),
                        next_hop.read().get_destination().read().get_full_name(),
                        route_to,
                        amount,
                        fee
                    );
                    next_hop.write().transfer_funds(
                        chain_id,
                        address,
                        client_order_id,
                        route_from,
                        route_to,
                        amount,
                    )
                }
            }
        }
    }
}

impl IntoObservableSingle<CollateralTransferEvent> for CollateralRouter {
    fn get_single_observer_mut(&mut self) -> &mut SingleObserver<CollateralTransferEvent> {
        &mut self.observer
    }
}

#[cfg(test)]
pub mod test_util {
    use eyre::Result;
    use std::sync::Arc;

    use parking_lot::RwLock;

    use crate::core::{
        bits::{Address, Amount, ClientOrderId, Symbol},
        functional::{IntoObservableSingle, PublishSingle, SingleObserver},
    };

    use super::{CollateralBridge, CollateralDesignation, CollateralRouterEvent};

    pub struct MockCollateralDesignation {
        pub type_: Symbol,
        pub name: Symbol,
        pub collateral_symbol: Symbol,
        pub full_name: Symbol,
        pub balance: Amount,
    }

    impl CollateralDesignation for MockCollateralDesignation {
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
            self.balance
        }
    }

    pub enum MockCollateralBridgeInternalEvent {
        TransferFunds {
            chain_id: u32,
            address: Address,
            client_order_id: ClientOrderId,
            route_from: Symbol,
            route_to: Symbol,
            amount: Amount,
        },
    }

    pub struct MockCollateralBridge {
        observer: SingleObserver<CollateralRouterEvent>,
        pub implementor: SingleObserver<MockCollateralBridgeInternalEvent>,
        pub source: Arc<RwLock<MockCollateralDesignation>>,
        pub destination: Arc<RwLock<MockCollateralDesignation>>,
    }

    impl MockCollateralBridge {
        pub fn new(
            source: Arc<RwLock<MockCollateralDesignation>>,
            destination: Arc<RwLock<MockCollateralDesignation>>,
        ) -> Self {
            Self {
                observer: SingleObserver::new(),
                implementor: SingleObserver::new(),
                source,
                destination,
            }
        }

        pub fn notify_collateral_router_event(
            &self,
            chain_id: u32,
            address: Address,
            client_order_id: ClientOrderId,
            route_from: Symbol,
            route_to: Symbol,
            amount: Amount,
            fee: Amount,
        ) {
            self.observer
                .publish_single(CollateralRouterEvent::HopComplete {
                    chain_id,
                    address,
                    client_order_id,
                    source: self.source.read().get_full_name(),
                    destination: self.destination.read().get_full_name(),
                    route_from,
                    route_to,
                    amount,
                    fee,
                });
        }
    }

    impl IntoObservableSingle<CollateralRouterEvent> for MockCollateralBridge {
        fn get_single_observer_mut(&mut self) -> &mut SingleObserver<CollateralRouterEvent> {
            &mut self.observer
        }
    }

    impl CollateralBridge for MockCollateralBridge {
        fn get_source(&self) -> Arc<RwLock<dyn CollateralDesignation>> {
            (self.source).clone() as Arc<RwLock<dyn CollateralDesignation>>
        }

        fn get_destination(&self) -> Arc<RwLock<dyn CollateralDesignation>> {
            (self.destination).clone() as Arc<RwLock<dyn CollateralDesignation>>
        }

        fn transfer_funds(
            &self,
            chain_id: u32,
            address: Address,
            client_order_id: ClientOrderId,
            route_from: Symbol,
            route_to: Symbol,
            amount: Amount,
        ) -> Result<()> {
            self.implementor
                .publish_single(MockCollateralBridgeInternalEvent::TransferFunds {
                    chain_id,
                    address,
                    client_order_id,
                    route_from,
                    route_to,
                    amount,
                });
            Ok(())
        }
    }
}
