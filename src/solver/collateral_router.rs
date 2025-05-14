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

pub trait TradingDesignation: Send + Sync {
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

pub enum TradingDesignationBridgeEvent {
    TransferComplete {
        chain_id: u32,
        address: Address,
        client_order_id: ClientOrderId,
        source: Symbol,
        destination: Symbol,
        amount: Amount,
        fee: Amount,
    },
}

pub trait TradingDesignationBridge: Send + Sync {
    /// e.g. EVM:ARBITRUM:USDC
    fn get_source(&self) -> Arc<RwLock<dyn TradingDesignation>>;

    /// e.g. BINANCE:1:USDT
    fn get_destination(&self) -> Arc<RwLock<dyn TradingDesignation>>;

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
        amount: Amount,
    );
}

pub struct CollateralRouter {
    observer: SingleObserver<TradingDesignationBridgeEvent>,
    bridges: HashMap<(Symbol, Symbol), Arc<RwLock<dyn TradingDesignationBridge>>>,
}

impl CollateralRouter {
    pub fn new() -> Self {
        Self {
            observer: SingleObserver::new(),
            bridges: HashMap::new(),
        }
    }

    pub fn add_bridge(&mut self, bridge: Arc<RwLock<dyn TradingDesignationBridge>>) -> Result<()> {
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
            .ok_or_eyre("Duplicate designation ID")?;
        Ok(())
    }

    pub fn transfer_funds_todo(&self, collateral_management: CollateralManagement, amount: Amount) {
        if let Some((_, bridge)) = self.bridges.iter().find(|_| true) {
            bridge.write().transfer_funds(
                collateral_management.chain_id,
                collateral_management.address,
                collateral_management.client_order_id,
                amount,
            );
        }
    }

    pub fn handle_trading_bridge_event(
        &mut self,
        event: TradingDesignationBridgeEvent,
    ) -> Result<()> {
        match event {
            TradingDesignationBridgeEvent::TransferComplete {
                chain_id,
                address,
                client_order_id,
                source,
                destination,
                amount,
                fee,
            } => {
                println!(
                    "Route Complete for {} {} {}: {} => {} {:0.5} {:0.5}",
                    chain_id, address, client_order_id, source, destination, amount, fee
                );

                // Republish for now
                // TODO: follow the route, and publish only once end-of-route reached
                self.observer
                    .publish_single(TradingDesignationBridgeEvent::TransferComplete {
                        chain_id,
                        address,
                        client_order_id,
                        source,
                        destination,
                        amount,
                        fee,
                    });
            }
        }
        Ok(())
    }
}

impl IntoObservableSingle<TradingDesignationBridgeEvent> for CollateralRouter {
    fn get_single_observer_mut(&mut self) -> &mut SingleObserver<TradingDesignationBridgeEvent> {
        &mut self.observer
    }
}

#[cfg(test)]
pub mod test_util {
    use std::sync::Arc;

    use parking_lot::RwLock;

    use crate::core::{
        bits::{Address, Amount, ClientOrderId, Symbol},
        functional::{IntoObservableSingle, PublishSingle, SingleObserver},
    };

    use super::{TradingDesignation, TradingDesignationBridge, TradingDesignationBridgeEvent};

    pub struct MockTradingDesignation {
        pub type_: Symbol,
        pub name: Symbol,
        pub collateral_symbol: Symbol,
        pub full_name: Symbol,
        pub balance: Amount,
    }

    impl TradingDesignation for MockTradingDesignation {
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

    pub enum MockTradingDesignationBridgeInternalEvent {
        TransferFunds {
            chain_id: u32,
            address: Address,
            client_order_id: ClientOrderId,
            amount: Amount,
        },
    }

    pub struct MockTradingDesignationBridge {
        observer: SingleObserver<TradingDesignationBridgeEvent>,
        pub implementor: SingleObserver<MockTradingDesignationBridgeInternalEvent>,
        pub source: Arc<RwLock<MockTradingDesignation>>,
        pub destination: Arc<RwLock<MockTradingDesignation>>,
    }

    impl MockTradingDesignationBridge {
        pub fn new(
            source: Arc<RwLock<MockTradingDesignation>>,
            destination: Arc<RwLock<MockTradingDesignation>>,
        ) -> Self {
            Self {
                observer: SingleObserver::new(),
                implementor: SingleObserver::new(),
                source,
                destination,
            }
        }

        pub fn notify_trading_designation_bridge_event(
            &self,
            chain_id: u32,
            address: Address,
            client_order_id: ClientOrderId,
            amount: Amount,
            fee: Amount,
        ) {
            self.observer
                .publish_single(TradingDesignationBridgeEvent::TransferComplete {
                    chain_id,
                    address,
                    client_order_id,
                    source: self.source.read().get_full_name(),
                    destination: self.destination.read().get_full_name(),
                    amount,
                    fee,
                });
        }
    }

    impl IntoObservableSingle<TradingDesignationBridgeEvent> for MockTradingDesignationBridge {
        fn get_single_observer_mut(
            &mut self,
        ) -> &mut SingleObserver<TradingDesignationBridgeEvent> {
            &mut self.observer
        }
    }

    impl TradingDesignationBridge for MockTradingDesignationBridge {
        fn get_source(&self) -> Arc<RwLock<dyn TradingDesignation>> {
            (self.source).clone() as Arc<RwLock<dyn TradingDesignation>>
        }

        fn get_destination(&self) -> Arc<RwLock<dyn TradingDesignation>> {
            (self.destination).clone() as Arc<RwLock<dyn TradingDesignation>>
        }

        fn transfer_funds(
            &self,
            chain_id: u32,
            address: Address,
            client_order_id: ClientOrderId,
            amount: Amount,
        ) {
            self.implementor.publish_single(
                MockTradingDesignationBridgeInternalEvent::TransferFunds {
                    chain_id,
                    address,
                    client_order_id,
                    amount,
                },
            );
        }
    }
}
