use std::{
    collections::HashMap,
    sync::{Arc, RwLock as ComponentLock},
};

use chrono::{DateTime, Utc};
use itertools::Itertools;

use eyre::{eyre, OptionExt, Result};

use symm_core::core::{
    bits::{Address, Amount, ClientOrderId, Side, Symbol},
    functional::{IntoObservableSingle, IntoObservableSingleVTable, PublishSingle, SingleObserver},
};


pub struct CollateralRouter {
    observer: SingleObserver<CollateralTransferEvent>,
    bridges: HashMap<(Symbol, Symbol), Arc<ComponentLock<dyn CollateralBridge>>>,
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

    pub fn add_bridge(&mut self, bridge: Arc<ComponentLock<dyn CollateralBridge>>) -> Result<()> {
        let key = (|| -> Result<(Symbol, Symbol)> {
            let bridge = bridge
                .read()
                .map_err(|e| eyre!("Failed to access bridge {}", e))?;
            Ok((
                bridge
                    .get_source()
                    .read()
                    .map_err(|e| eyre!("Failed to access source {}", e))?
                    .get_full_name(),
                bridge
                    .get_destination()
                    .read()
                    .map_err(|e| eyre!("Failed to access destination {}", e))?
                    .get_full_name(),
            ))
        })()?;
        self.bridges
            .insert(key, bridge)
            .is_none()
            .then_some(())
            .ok_or_eyre("Duplicate designation ID")
    }

    pub fn get_bridges(&self) -> Vec<Arc<ComponentLock<dyn CollateralBridge>>> {
        self.bridges.values().cloned().collect_vec()
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
        chain_id: u32,
        address: Address,
        client_order_id: ClientOrderId,
        side: Side,
        amount: Amount,
    ) -> Result<()> {
        //
        // TODO: Scan CollatearalManagement and tell:
        // - what are the final destinations for collateral (multiple destinations)
        //
        // Note that for now we have single "default" destination
        //
        if let Side::Sell = side {
            // Sell requires us to pull cash from destination back to source
            todo!("Collateral routing for sell is not yet supported");
        }

        let transfer_from = self
            .chain_sources
            .get(&chain_id)
            .ok_or_eyre("Failed to find source")?;

        let transfer_to = self
            .default_destination
            .clone()
            .ok_or_eyre("Failed to find destination")?;

        let first_hop = self.next_hop(transfer_from, transfer_from, &transfer_to)?;

        first_hop
            .write()
            .map_err(|e| eyre!("Failed to access next hop {}", e))?
            .transfer_funds(
                chain_id,
                address,
                client_order_id,
                transfer_from.clone(),
                transfer_to.clone(),
                amount,
                Amount::ZERO, // we could charge some initial fee too!
            )
    }

    fn next_hop(
        &self,
        source: &Symbol,
        route_from: &Symbol,
        route_to: &Symbol,
    ) -> Result<&Arc<ComponentLock<dyn CollateralBridge>>> {
        let route = self
            .routes
            .iter()
            .find(|route| match (route.first(), route.last()) {
                (Some(first), Some(last)) => route_from.eq(first) && route_to.eq(last),
                _ => false,
            })
            .ok_or_eyre("Route not found")?;

        tracing::info!(
            "(collateral-router) Found route: {}",
            route.iter().join(", ")
        );

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
                timestamp,
                source,
                destination,
                route_from,
                route_to,
                amount,
                fee,
            } => {
                if route_to.eq(&destination) {
                    tracing::info!(
                        "(collateral-router) Route Complete for [{}:{}] {}: {} => {} {:0.5} {:0.5}",
                        chain_id,
                        address,
                        client_order_id,
                        route_from,
                        route_to,
                        amount,
                        fee
                    );
                    self.observer
                        .publish_single(CollateralTransferEvent::TransferComplete {
                            chain_id,
                            address,
                            client_order_id,
                            timestamp,
                            transfer_from: route_from,
                            transfer_to: route_to,
                            amount,
                            fee,
                        });
                    Ok(())
                } else {
                    let next_hop = self.next_hop(&destination, &route_from, &route_to)?;
                    let next_hop = next_hop
                        .write()
                        .map_err(|e| eyre!("Failed to access next hop {}", e))?;
                    tracing::info!(
                        "(collateral-router) Route Hop for [{}:{}] {}: ({}) {} .. [{}] => {} ({}) {:0.5} {:0.5}",
                        chain_id,
                        address,
                        client_order_id,
                        route_from,
                        source,
                        next_hop.get_source().read().map(|x| x.get_full_name()).unwrap_or_default(),
                        next_hop.get_destination().read().map(|x| x.get_full_name()).unwrap_or_default(),
                        route_to,
                        amount,
                        fee
                    );
                    next_hop.transfer_funds(
                        chain_id,
                        address,
                        client_order_id,
                        route_from,
                        route_to,
                        amount,
                        fee,
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
    use chrono::{DateTime, Utc};
    use eyre::Result;
    use itertools::Itertools;
    use rust_decimal::dec;
    use std::{
        collections::HashMap,
        sync::{mpsc::Sender, Arc, RwLock as ComponentLock},
    };

    use symm_core::core::{
        bits::{Address, Amount, ClientOrderId, Symbol},
        functional::{
            IntoObservableSingle, IntoObservableSingleVTable, NotificationHandlerOnce,
            PublishSingle, SingleObserver,
        },
    };

    use super::{CollateralBridge, CollateralDesignation, CollateralRouter, CollateralRouterEvent};

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
            cumulative_fee: Amount,
        },
    }

    pub struct MockCollateralBridge {
        observer: SingleObserver<CollateralRouterEvent>,
        pub implementor: SingleObserver<MockCollateralBridgeInternalEvent>,
        pub source: Arc<ComponentLock<MockCollateralDesignation>>,
        pub destination: Arc<ComponentLock<MockCollateralDesignation>>,
    }

    impl MockCollateralBridge {
        pub fn new(
            source: Arc<ComponentLock<MockCollateralDesignation>>,
            destination: Arc<ComponentLock<MockCollateralDesignation>>,
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
            timestamp: DateTime<Utc>,
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
                    timestamp,
                    source: self.source.read().unwrap().get_full_name(),
                    destination: self.destination.read().unwrap().get_full_name(),
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

    impl IntoObservableSingleVTable<CollateralRouterEvent> for MockCollateralBridge {
        fn set_observer(
            &mut self,
            observer: Box<dyn NotificationHandlerOnce<CollateralRouterEvent>>,
        ) {
            self.get_single_observer_mut().set_observer(observer);
        }
    }

    impl CollateralBridge for MockCollateralBridge {
        fn get_source(&self) -> Arc<ComponentLock<dyn CollateralDesignation>> {
            (self.source).clone() as Arc<ComponentLock<dyn CollateralDesignation>>
        }

        fn get_destination(&self) -> Arc<ComponentLock<dyn CollateralDesignation>> {
            (self.destination).clone() as Arc<ComponentLock<dyn CollateralDesignation>>
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
            self.implementor
                .publish_single(MockCollateralBridgeInternalEvent::TransferFunds {
                    chain_id,
                    address,
                    client_order_id,
                    route_from,
                    route_to,
                    amount,
                    cumulative_fee,
                });
            Ok(())
        }
    }

    /// Make mocked designation
    pub fn make_mock_designation(full_name: &str) -> Arc<ComponentLock<MockCollateralDesignation>> {
        let (t, n, c) = full_name.split(":").collect_tuple().unwrap();
        Arc::new(ComponentLock::new(MockCollateralDesignation {
            type_: t.to_owned().into(),
            name: n.to_owned().into(),
            collateral_symbol: c.to_owned().into(),
            full_name: full_name.to_owned().into(),
            balance: dec!(0.0),
        }))
    }

    /// Make mocked bridge w/o implementation
    pub fn make_mock_bridge(
        from: &Arc<ComponentLock<MockCollateralDesignation>>,
        to: &Arc<ComponentLock<MockCollateralDesignation>>,
    ) -> Arc<ComponentLock<MockCollateralBridge>> {
        Arc::new(ComponentLock::new(MockCollateralBridge::new(
            from.clone(),
            to.clone(),
        )))
    }

    /// Implement mocked bridge using fee calculation function
    pub fn implement_mock_bridge(
        tx: &Sender<Box<dyn FnOnce() + Send + Sync>>,
        bridge: &Arc<ComponentLock<MockCollateralBridge>>,
        router: &Arc<ComponentLock<CollateralRouter>>,
        calculate_fee: &Arc<dyn Fn(Amount, Amount) -> (Amount, Amount) + Send + Sync>,
    ) {
        // Send bridge events into router
        let tx_clone = tx.clone();
        let router_weak = Arc::downgrade(&router);
        bridge
            .write()
            .unwrap()
            .get_single_observer_mut()
            .set_observer_fn(move |e| {
                let router = router_weak.upgrade().unwrap();
                tx_clone
                    .send(Box::new(move || {
                        router
                            .write()
                            .unwrap()
                            .handle_collateral_router_event(e)
                            .unwrap();
                    }))
                    .unwrap();
            });

        // Implement bridge
        let tx_clone = tx.clone();
        let bridge_weak = Arc::downgrade(bridge);
        let calculate_fee = calculate_fee.clone();
        bridge
            .write()
            .unwrap()
            .implementor
            .set_observer_fn(move |e| {
                let bridge = bridge_weak.upgrade().unwrap();
                let calculate_fee = calculate_fee.clone();
                tx_clone
                    .send(Box::new(move || match e {
                        MockCollateralBridgeInternalEvent::TransferFunds {
                            chain_id,
                            address,
                            client_order_id,
                            route_from,
                            route_to,
                            amount,
                            cumulative_fee,
                        } => {
                            let (amount, cumulative_fee) = calculate_fee(amount, cumulative_fee);
                            bridge.write().unwrap().notify_collateral_router_event(
                                chain_id,
                                address,
                                client_order_id,
                                Utc::now(),
                                route_from,
                                route_to,
                                amount,
                                cumulative_fee,
                            );
                        }
                    }))
                    .unwrap();
            });

        // Add bridge to the router
        router.write().unwrap().add_bridge(bridge.clone()).unwrap();
    }

    /// Create mocked router for unit-tests
    pub fn build_test_router(
        tx: &Sender<Box<dyn FnOnce() + Send + Sync>>,
        designations: &[&str],
        bridges: &[(&str, &str)],
        routes: &[&[&str]],
        source_chain_map: &[(u32, &str)],
        default_designation: &str,
        calculate_fee: impl Fn(Amount, Amount) -> (Amount, Amount) + Send + Sync + 'static,
    ) -> Arc<ComponentLock<CollateralRouter>> {
        let designations: HashMap<String, _> = HashMap::from_iter(
            designations
                .iter()
                .map(|&n| (n.to_owned(), make_mock_designation(n))),
        );

        let bridges = bridges
            .iter()
            .map(|(a, b)| {
                make_mock_bridge(designations.get(*a).unwrap(), designations.get(*b).unwrap())
            })
            .collect_vec();

        let router = Arc::new(ComponentLock::new(CollateralRouter::new()));

        let calculate_fee: Arc<dyn Fn(Amount, Amount) -> (Amount, Amount) + Send + Sync + 'static> =
            Arc::new(calculate_fee);

        for bridge in bridges.iter() {
            implement_mock_bridge(tx, bridge, &router, &calculate_fee);
        }

        for (chain_id, source) in source_chain_map {
            router
                .write()
                .unwrap()
                .add_chain_source(*chain_id, source.to_owned().into())
                .unwrap();
        }

        router
            .write()
            .unwrap()
            .set_default_destination(default_designation.to_owned().into())
            .unwrap();

        for &route in routes {
            let route = route
                .iter()
                .map(|n| n.to_owned())
                .map(Symbol::from)
                .collect_vec();
            router.write().unwrap().add_route(&route).unwrap();
        }

        router
    }
}

#[cfg(test)]
mod test {
    use rust_decimal::dec;

    use test_case::test_case;

    use crate::collateral::collateral_router::test_util::build_test_router;
    use symm_core::{
        assert_decimal_approx_eq,
        core::{
            bits::Side,
            functional::IntoObservableSingle,
            test_util::{
                flag_mock_atomic_bool, get_mock_address_1, get_mock_atomic_bool_pair,
                get_mock_defer_channel, run_mock_deferred, test_mock_atomic_bool,
            },
        },
    };

    use super::CollateralTransferEvent;

    /// Test Collateral Router
    /// -------
    /// Collateral Router routes collateral by creating path from multiple
    /// bridges.  It stores routing table that tells which bridges need to be
    /// connected to obtain path from source to destination.
    ///
    /// This tests two paths to check that router will select correct route
    /// depending on source chain.
    ///
    #[test_case(1, "T1:N1:C1"; "Take first route")]
    #[test_case(2, "T2:N2:C2"; "Take second route")]
    fn test_collateral_router(expected_chain_id: u32, expected_from: &'static str) {
        let (event_get, event_set) = get_mock_atomic_bool_pair();
        let (tx, rx) = get_mock_defer_channel();

        let router = build_test_router(
            &tx,
            &["T1:N1:C1", "T2:N2:C2", "T3:N3:C3", "T4:N4:C4"],
            &[
                ("T1:N1:C1", "T3:N3:C3"),
                ("T2:N2:C2", "T3:N3:C3"),
                ("T3:N3:C3", "T4:N4:C4"),
            ],
            &[
                &["T1:N1:C1", "T3:N3:C3", "T4:N4:C4"],
                &["T2:N2:C2", "T3:N3:C3", "T4:N4:C4"],
            ],
            &[(1, "T1:N1:C1"), (2, "T2:N2:C2")],
            "T4:N4:C4",
            |amount, cumulative_fee| {
                let fee = dec!(0.5);
                let cumulative_fee = cumulative_fee + fee;
                (amount - fee, cumulative_fee)
            },
        );

        router
            .write()
            .unwrap()
            .get_single_observer_mut()
            .set_observer_fn(move |e| match e {
                CollateralTransferEvent::TransferComplete {
                    chain_id,
                    address,
                    client_order_id,
                    timestamp: _,
                    transfer_from,
                    transfer_to,
                    amount,
                    fee,
                } => {
                    let tolerance = dec!(0.01);

                    assert_eq!(chain_id, expected_chain_id);
                    assert_eq!(address, get_mock_address_1());
                    assert_eq!(client_order_id, "C-1".into());

                    assert_eq!(transfer_from, expected_from.to_owned());
                    assert_eq!(transfer_to, "T4:N4:C4".to_owned());

                    assert_decimal_approx_eq!(amount, dec!(999.0), tolerance);
                    assert_decimal_approx_eq!(fee, dec!(1.0), tolerance);

                    flag_mock_atomic_bool(&event_set);
                }
            });

        // Make test transfer
        // It will be coming from given Chain ID, and
        // it will be routed to final designation.
        router
            .write()
            .unwrap()
            .transfer_collateral(
                expected_chain_id,
                get_mock_address_1(),
                "C-1".into(),
                Side::Buy,
                dec!(1000.0),
            )
            .unwrap();

        run_mock_deferred(&rx);

        assert!(test_mock_atomic_bool(&event_get));
    }
}
