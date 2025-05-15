use std::{
    collections::{HashMap, VecDeque},
    sync::Arc,
};

use chrono::{DateTime, TimeDelta, Utc};
use itertools::{Either, Itertools};
use parking_lot::RwLock;

use eyre::{OptionExt, Result};
use safe_math::safe;

use crate::{
    core::{
        bits::{Address, Amount, ClientOrderId, PaymentId, Side},
        decimal_ext::DecimalExt,
        functional::{IntoObservableSingle, PublishSingle, SingleObserver},
    },
    solver::solver::{CollateralManagement, SetSolverOrderStatus},
};

use super::collateral_router::{CollateralRouter, CollateralTransferEvent};

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
    NotEnoughFunds,
}

struct CollateralSpend {
    /// Client Order ID
    client_order_id: ClientOrderId,

    /// Payment ID of this spend
    payment_id: PaymentId,

    /// Amount spent
    amount: Amount,

    /// Time of spend
    timestamp: DateTime<Utc>,
}

struct CollateralLot {
    /// Payment ID of this funding (credit/debit)
    payment_id: PaymentId,

    /// Total amount of funding (unconfirmed)
    unconfirmed_amount: Amount,

    /// Total amount of funding (ready to trade)
    ready_amount: Amount,

    /// Total amount spent (spent on trade)
    spent_amount: Amount,

    /// Time of funding
    created_timestamp: DateTime<Utc>,

    /// Time of the last update
    last_update_timestamp: DateTime<Utc>,

    /// Collateral spent for orders
    spends: Vec<CollateralSpend>,
}

impl CollateralLot {
    pub fn new(payment_id: PaymentId, amount: Amount, timestamp: DateTime<Utc>) -> Self {
        Self {
            payment_id,
            unconfirmed_amount: amount,
            ready_amount: Amount::ZERO,
            spent_amount: Amount::ZERO,
            created_timestamp: timestamp,
            last_update_timestamp: timestamp,
            spends: Vec::new(),
        }
    }
}

struct CollateralSide {
    /// Total amount of funding (unconfirmed)
    unconfirmed_balance: Amount,

    /// Total amount of funding (ready to trade)
    ready_balance: Amount,

    /// Total amount of funding (pre-authorized)
    preauth_balance: Amount,

    /// Total amount spent (spent on trade)
    spent_balance: Amount,

    /// Lots open w/ some non-zero balance (unconfirmed + ready)
    open_lots: VecDeque<CollateralLot>,

    /// Lots closed w/ all balance spent
    closed_lots: VecDeque<CollateralLot>,
}

impl CollateralSide {
    pub fn new() -> Self {
        Self {
            unconfirmed_balance: Amount::ZERO,
            ready_balance: Amount::ZERO,
            preauth_balance: Amount::ZERO,
            spent_balance: Amount::ZERO,
            open_lots: VecDeque::new(),
            closed_lots: VecDeque::new(),
        }
    }
}

struct CollateralPosition {
    /// Chain ID
    chain_id: u32,

    /// On-chain address of the User
    address: Address,

    /// Credits
    side_cr: CollateralSide,

    /// Debits
    side_dr: CollateralSide,

    /// Time position was created
    created_timestamp: DateTime<Utc>,

    /// Last time we updated this position
    last_update_timestamp: DateTime<Utc>,
}

impl CollateralPosition {
    pub fn new(chain_id: u32, address: Address, timestmap: DateTime<Utc>) -> Self {
        Self {
            chain_id,
            address,
            side_cr: CollateralSide::new(),
            side_dr: CollateralSide::new(),
            created_timestamp: timestmap,
            last_update_timestamp: timestmap,
        }
    }
}

pub trait CollateralManagerHost: SetSolverOrderStatus {
    fn get_next_payment_id(&self) -> PaymentId;
}

pub struct CollateralManager {
    observer: SingleObserver<CollateralEvent>,
    router: Arc<RwLock<CollateralRouter>>,
    client_funds: HashMap<(u32, Address), Arc<RwLock<CollateralPosition>>>,
    collateral_management_requests: VecDeque<CollateralManagement>,
    zero_threshold: Amount,
    fund_wait_period: TimeDelta,
}

impl CollateralManager {
    pub fn new(
        router: Arc<RwLock<CollateralRouter>>,
        zero_threshold: Amount,
        fund_wait_period: TimeDelta,
    ) -> Self {
        Self {
            observer: SingleObserver::new(),
            router,
            client_funds: HashMap::new(),
            collateral_management_requests: VecDeque::new(),
            zero_threshold,
            fund_wait_period,
        }
    }

    pub fn process_credits(
        &mut self,
        _host: &dyn CollateralManagerHost,
        _timestamp: DateTime<Utc>,
    ) -> Result<()> {
        let requests = VecDeque::from_iter(self.collateral_management_requests.drain(..));

        let (ready_to_route, check_later): (Vec<_>, Vec<_>) =
            requests.into_iter().partition_map(|request| {
                if let Some(position) = self.get_position(request.chain_id, &request.address) {
                    let position_read = position.read();
                    let unconfirmed_balance = match request.side {
                        Side::Buy => position_read.side_cr.unconfirmed_balance,
                        Side::Sell => position_read.side_dr.unconfirmed_balance,
                    };
                    if unconfirmed_balance < request.collateral_amount {
                        Either::Right(request)
                    } else {
                        Either::Left((request, unconfirmed_balance))
                    }
                } else {
                    Either::Right(request)
                }
            });

        self.collateral_management_requests.extend(check_later);

        for (request, unconfirmed_balance) in ready_to_route {
            self.router.write().transfer_collateral(
                request.chain_id,
                request.address,
                request.client_order_id,
                request.side,
                unconfirmed_balance,
            );
        }
        Ok(())
    }

    pub fn manage_collateral(&mut self, collateral_management: CollateralManagement) {
        println!(
            "(collateral-manager) ManageCollateral for {} {}",
            collateral_management.address, collateral_management.client_order_id
        );
        self.collateral_management_requests
            .push_back(collateral_management);
    }

    pub fn handle_deposit(
        &mut self,
        host: &dyn CollateralManagerHost,
        chain_id: u32,
        address: Address,
        amount: Amount,
        timestamp: DateTime<Utc>,
    ) -> Result<()> {
        println!(
            "(collateral-manager) Deposit from [{}:{}] {:0.5}",
            chain_id, address, amount
        );
        let collateral_position = self.add_position(chain_id, address, timestamp);
        let mut collateral_position_write = collateral_position.write();

        collateral_position_write
            .side_cr
            .open_lots
            .push_back(CollateralLot::new(
                host.get_next_payment_id(),
                amount,
                timestamp,
            ));

        collateral_position_write.side_cr.unconfirmed_balance =
            safe!(collateral_position_write.side_cr.unconfirmed_balance + amount)
                .ok_or_eyre("Math Problem")?;

        collateral_position_write.last_update_timestamp = timestamp;
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
        println!(
            "(collateral-manager) Withdrawal from [{}:{}] {:0.5}",
            chain_id, address, amount
        );
        let collateral_position = self.add_position(chain_id, address, timestamp);
        let mut collateral_position_write = collateral_position.write();

        collateral_position_write
            .side_dr
            .open_lots
            .push_back(CollateralLot::new(
                host.get_next_payment_id(),
                amount,
                timestamp,
            ));

        collateral_position_write.side_cr.unconfirmed_balance =
            safe!(collateral_position_write.side_cr.unconfirmed_balance + amount)
                .ok_or_eyre("Math Problem")?;

        collateral_position_write.last_update_timestamp = timestamp;
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
        side: Side,
        amount_payable: Amount,
    ) -> Result<PaymentStatus> {
        println!(
            "(collateral-manager) PreAuth Payment for {} {:0.5}",
            address, amount_payable
        );
        if let Side::Sell = side {
            todo!("Collateral management for sell is not supported yet");
        }

        let funds = self
            .get_position(chain_id, &address)
            .ok_or_eyre("Failed to find position")?;

        (|| {
            let mut funds_upread = funds.upgradable_read();
            let balance_remaining = safe!(
                safe!(
                    funds_upread.side_cr.ready_balance
                        - safe!(
                            funds_upread.side_dr.unconfirmed_balance
                                + funds_upread.side_dr.ready_balance
                        )?
                )? - amount_payable
            )?;

            if balance_remaining < -self.zero_threshold {
                Some(PaymentStatus::NotEnoughFunds)
            } else {
                let payment_id = host.get_next_payment_id();

                let ready_balance = safe!(funds_upread.side_cr.ready_balance - amount_payable)?;
                let preauth_balance = safe!(funds_upread.side_cr.preauth_balance + amount_payable)?;

                funds_upread.with_upgraded(|funds_write| {
                    funds_write.side_cr.ready_balance = ready_balance;
                    funds_write.side_cr.preauth_balance = preauth_balance;
                });

                Some(PaymentStatus::Approved { payment_id })
            }
        })()
        .ok_or_eyre("Math Problem")
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

        // TODO: Add spend to lots

        Ok(())
    }

    fn add_position(
        &mut self,
        chain_id: u32,
        address: Address,
        timestamp: DateTime<Utc>,
    ) -> Arc<RwLock<CollateralPosition>> {
        self.client_funds
            .entry((chain_id, address))
            .or_insert_with(|| {
                Arc::new(RwLock::new(CollateralPosition::new(
                    chain_id, address, timestamp,
                )))
            })
            .clone()
    }

    fn get_position(
        &self,
        chain_id: u32,
        address: &Address,
    ) -> Option<&Arc<RwLock<CollateralPosition>>> {
        self.client_funds.get(&(chain_id, address.clone()))
    }

    pub fn handle_collateral_transfer_event(
        &mut self,
        event: CollateralTransferEvent,
    ) -> Result<()> {
        match event {
            CollateralTransferEvent::TransferComplete {
                chain_id,
                address,
                client_order_id,
                transfer_from,
                transfer_to,
                amount,
                fee,
            } => {
                println!(
                    "(collateral-manager) Transfer Complete for {} {} {}: {} => {} {:0.5} {:0.5}",
                    chain_id, address, client_order_id, transfer_from, transfer_to, amount, fee
                );
                let funds = self
                    .get_position(chain_id, &address)
                    .ok_or_eyre("Failed to find position")?;

                (|| {
                    let mut funds_upread = funds.upgradable_read();

                    let ready_balance = safe!(funds_upread.side_cr.ready_balance + amount)?;
                    let unconfirmed_balance =
                        safe!(funds_upread.side_cr.unconfirmed_balance - amount)?;

                    funds_upread.with_upgraded(|funds_write| {
                        funds_write.side_cr.ready_balance = ready_balance;
                        funds_write.side_cr.unconfirmed_balance = unconfirmed_balance;
                    });

                    Some(())
                })()
                .ok_or_eyre("Math Problem")?;

                self.observer
                    .publish_single(CollateralEvent::CollateralReady {
                        chain_id,
                        address,
                        client_order_id,
                        collateral_amount: amount,
                        fee,
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
