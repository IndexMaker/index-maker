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

pub enum CollateralEvent {
    CollateralReady {
        chain_id: u32,
        address: Address,
        client_order_id: ClientOrderId,
        collateral_amount: Amount,
        fee: Amount,
    },
}

pub enum PaymentStatus {
    Approved { payment_id: PaymentId },
    Denied,
}

struct CollateralTransaction {
    payment_id: PaymentId,
    amount: Amount,
    timestamp: DateTime<Utc>,
}

pub struct CollateralPosition {
    /// On-chain address of the User
    address: Address,

    /// Total balance of credits less debits in force
    pub balance: Amount,

    /// Total amount of pending credits
    pending_cr: Amount,

    /// Total amount of pending debits
    pending_dr: Amount,

    /// Pending credits
    transactions_cr: VecDeque<CollateralTransaction>,

    /// Pending debits
    transactions_dr: VecDeque<CollateralTransaction>,

    /// Completed credits and debits
    completed_transactions: VecDeque<CollateralTransaction>,

    /// Last time we updated this position
    last_update_timestamp: DateTime<Utc>,
}

impl CollateralPosition {
    fn new(address: Address) -> Self {
        Self {
            address,
            balance: Amount::ZERO,
            pending_cr: Amount::ZERO,
            pending_dr: Amount::ZERO,
            transactions_cr: VecDeque::new(),
            transactions_dr: VecDeque::new(),
            completed_transactions: VecDeque::new(),
            last_update_timestamp: Default::default(),
        }
    }
}

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

pub trait CollateralManagerHost: SetSolverOrderStatus {
    fn get_next_payment_id(&self) -> PaymentId;
}

pub struct CollateralManager {
    observer: SingleObserver<CollateralEvent>,
    bridges: HashMap<(Symbol, Symbol), Arc<RwLock<dyn TradingDesignationBridge>>>,
    client_funds: HashMap<Address, Arc<RwLock<CollateralPosition>>>,
    ready_funds: VecDeque<Arc<RwLock<CollateralPosition>>>,
    collateral_management_requests: VecDeque<CollateralManagement>,
    zero_threshold: Amount,
    fund_wait_period: TimeDelta,
}

impl CollateralManager {
    pub fn new(zero_threshold: Amount, fund_wait_period: TimeDelta) -> Self {
        Self {
            observer: SingleObserver::new(),
            bridges: HashMap::new(),
            client_funds: HashMap::new(),
            ready_funds: VecDeque::new(),
            collateral_management_requests: VecDeque::new(),
            zero_threshold,
            fund_wait_period,
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

    pub fn process_credits(
        &mut self,
        host: &dyn CollateralManagerHost,
        timestamp: DateTime<Utc>,
    ) -> Result<()> {
        let ready_timestamp = timestamp - self.fund_wait_period;
        let check_not_ready = |x: &CollateralTransaction| ready_timestamp < x.timestamp;
        let ready_funds = (|| {
            let res = self.ready_funds.iter().find_position(|p| {
                p.read()
                    .transactions_cr
                    .front()
                    .map(check_not_ready)
                    .unwrap_or(true)
            });
            if let Some((pos, _)) = res {
                self.ready_funds.drain(..pos)
            } else {
                self.ready_funds.drain(..)
            }
            .collect_vec()
        })();

        for fund in ready_funds {
            let mut fund_write = fund.write();
            let res = fund_write
                .transactions_cr
                .iter()
                .find_position(|x| check_not_ready(x));
            let completed = if let Some((pos, _)) = res {
                fund_write.transactions_cr.drain(..pos)
            } else {
                fund_write.transactions_cr.drain(..)
            }
            .collect_vec();
            fund_write.balance = completed
                .iter()
                .try_fold(fund_write.balance, |balance, tx| safe!(balance + tx.amount))
                .ok_or_eyre("Math Problem")?;
            fund_write.last_update_timestamp = ready_timestamp;
            fund_write.completed_transactions.extend(completed);
            println!(
                "New balance for {} {:0.5}",
                fund_write.address, fund_write.balance
            );
            // TODO: We would fire this event after all collateral management completes
            if let Some(unwrapped) = self.collateral_management_requests.pop_front() {
                if let Some((_, bridge)) = self.bridges.iter().find(|_| true) {
                    bridge.write().transfer_funds(
                        unwrapped.chain_id,
                        unwrapped.address,
                        unwrapped.client_order_id,
                        fund_write.balance,
                    );
                }
            }
        }
        Ok(())
    }

    pub fn handle_deposit(
        &mut self,
        host: &dyn CollateralManagerHost,
        chain_id: u32,
        address: Address,
        amount: Amount,
        timestamp: DateTime<Utc>,
    ) -> Result<()> {
        println!("Deposit form {} {} {:0.5}", chain_id, address, amount);
        let collateral_position = self
            .client_funds
            .entry(address)
            .or_insert_with(|| Arc::new(RwLock::new(CollateralPosition::new(address))))
            .clone();

        (|| -> Result<()> {
            let mut p = collateral_position.write();

            let payment_id = host.get_next_payment_id();
            p.transactions_cr.push_back(CollateralTransaction {
                payment_id,
                amount,
                timestamp,
            });
            p.pending_cr = safe!(p.pending_cr + amount).ok_or_eyre("Math Problem")?;
            p.last_update_timestamp = timestamp;
            Ok(())
        })()?;

        self.ready_funds.push_back(collateral_position);
        Ok(())
    }

    pub fn handle_withdrawal(
        &mut self,
        host: &dyn CollateralManagerHost,
        chain_id: u32,
        address: Address,
        amount: Amount,
        timestamp: DateTime<Utc>,
    ) -> Result<()> {
        println!("Withdrawal form {} {} {:0.5}", chain_id, address, amount);
        let collateral_position = self
            .client_funds
            .entry(address)
            .or_insert_with(|| Arc::new(RwLock::new(CollateralPosition::new(address))))
            .clone();

        (|| -> Result<()> {
            let mut p = collateral_position.write();

            let payment_id = host.get_next_payment_id();
            p.transactions_dr.push_back(CollateralTransaction {
                payment_id,
                amount,
                timestamp,
            });
            p.pending_dr = safe!(p.pending_dr + amount).ok_or_eyre("Math Problem")?;
            p.last_update_timestamp = timestamp;
            Ok(())
        })()?;

        self.ready_funds.push_back(collateral_position);
        Ok(())
    }

    /// Pre-Authorize Payment
    ///
    /// Note: For bigger Index Orders we will pre-authorize certain amount
    /// payable before we start processing Index Order. For smaller Index Orders
    /// we may have some margin to process them before payment is authorized.
    pub fn preauth_payment(
        &self,
        host: &dyn CollateralManagerHost,
        chain_id: u32,
        address: Address,
        amount_payable: Amount,
    ) -> Result<PaymentStatus> {
        println!("PreAuth Payment for {} {:0.5}", address, amount_payable);
        if let Some(funds) = self.get_funds(&address) {
            let mut funds_write = funds.write();

            let balance_remaining =
                safe!(funds_write.balance - amount_payable).ok_or_eyre("Math Problem")?;

            if balance_remaining < -self.zero_threshold {
                Ok(PaymentStatus::Denied)
            } else {
                // TODO! Don't just change numbers. Record a transaction!
                funds_write.balance = balance_remaining;
                let payment_id = host.get_next_payment_id();

                Ok(PaymentStatus::Approved { payment_id })
            }
        } else {
            Ok(PaymentStatus::Denied)
        }
    }

    /// Comfirm Payment
    ///
    /// Note: Once Index Order is fully-filled, we will calculate all the costs
    /// associated and we will charge user account that amount. Index Order is
    /// processed for as long as there is some collateral left, so that user
    /// will get the maxim amount of index token for the collateral they
    /// provided. The quantity that user receives depends on market dynamics.
    pub fn confirm_payment(
        &mut self,
        chain_id: u32,
        address: &Address,
        client_order_id: &ClientOrderId,
        payment_id: &PaymentId,
        amount_paid: Amount,
    ) -> Result<()> {
        // let funds = self
        //     .collateral_manager
        //     .read()
        //     .get_funds(&index_order.address)
        //     .cloned()
        //     .ok_or_eyre("Missing funds")?;

        // let mut funds_upread = funds.upgradable_read();

        // if funds_upread.balance < total_cost {
        //     Err(eyre!("Not enough funds"))?;
        // }

        // let new_balance = safe!(funds_upread.balance - total_cost).ok_or_eyre("Math Problem")?;
        // funds_upread.with_upgraded(|funds_write| funds_write.balance = new_balance);

        Ok(())
    }

    pub fn get_funds(&self, address: &Address) -> Option<&Arc<RwLock<CollateralPosition>>> {
        self.client_funds.get(address)
    }

    pub fn manage_collateral(&mut self, collateral_management: CollateralManagement) {
        println!(
            "ManageCollateral for {} {}",
            collateral_management.address, collateral_management.client_order_id
        );
        self.collateral_management_requests
            .push_back(collateral_management);
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
                    "Transfer Complete for {} {} {}: {} => {} {:0.5} {:0.5}",
                    chain_id, address, client_order_id, source, destination, amount, fee
                );
                self.observer
                    .publish_single(CollateralEvent::CollateralReady {
                        chain_id,
                        address,
                        client_order_id,
                        collateral_amount: amount,
                        fee
                    });
            }
        }
        Ok(())
    }
}

impl IntoObservableSingle<CollateralEvent> for CollateralManager {
    fn get_single_observer_mut(&mut self) -> &mut SingleObserver<CollateralEvent> {
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
