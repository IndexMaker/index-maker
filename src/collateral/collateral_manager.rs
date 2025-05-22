use std::{
    collections::{HashMap, VecDeque},
    sync::{Arc, RwLock as ComponentLock},
};

use chrono::{DateTime, Utc};
use itertools::FoldWhile::{Continue, Done};
use itertools::{Either, Itertools};
use parking_lot::RwLock;

use eyre::{eyre, OptionExt, Result};
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

pub enum PreAuthStatus {
    Approved { payment_id: PaymentId },
    NotEnoughFunds,
}
pub enum ConfirmStatus {
    Authorized,
    NotEnoughFunds,
}

pub enum CollateralEvent {
    CollateralReady {
        chain_id: u32,
        address: Address,
        client_order_id: ClientOrderId,
        collateral_amount: Amount,
        fee: Amount,
        timestamp: DateTime<Utc>,
    },
    PreAuthResponse {
        chain_id: u32,
        address: Address,
        client_order_id: ClientOrderId,
        amount_payable: Amount,
        timestamp: DateTime<Utc>,
        status: PreAuthStatus,
    },
    ConfirmResponse {
        chain_id: u32,
        address: Address,
        client_order_id: ClientOrderId,
        payment_id: PaymentId,
        amount_paid: Amount,
        timestamp: DateTime<Utc>,
        status: ConfirmStatus,
    },
}

struct CollateralSpend {
    /// Client Order ID
    client_order_id: ClientOrderId,

    /// Payment ID of this spend
    payment_id: PaymentId,

    /// Amount of funding (pre-authorized)
    preauth_amount: Amount,

    /// Amount spent
    spent_amount: Amount,

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

    /// Total amount of funding (pre-authorized)
    preauth_amount: Amount,

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
            preauth_amount: Amount::ZERO,
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
    open_lots: Vec<CollateralLot>,

    /// Lots closed w/ all balance spent
    closed_lots: VecDeque<CollateralLot>,

    /// Time position was created
    created_timestamp: DateTime<Utc>,

    /// Last time we updated this position
    last_update_timestamp: DateTime<Utc>,
}

impl CollateralSide {
    pub fn new(timestamp: DateTime<Utc>) -> Self {
        Self {
            unconfirmed_balance: Amount::ZERO,
            ready_balance: Amount::ZERO,
            preauth_balance: Amount::ZERO,
            spent_balance: Amount::ZERO,
            open_lots: Vec::new(),
            closed_lots: VecDeque::new(),
            created_timestamp: timestamp,
            last_update_timestamp: timestamp,
        }
    }

    pub fn open_lot(
        &mut self,
        payment_id: PaymentId,
        amount: Amount,
        timestamp: DateTime<Utc>,
    ) -> Result<()> {
        self.open_lots
            .push(CollateralLot::new(payment_id, amount, timestamp));

        self.unconfirmed_balance =
            safe!(self.unconfirmed_balance + amount).ok_or_eyre("Math Problem")?;

        self.last_update_timestamp = timestamp;

        Ok(())
    }

    pub fn add_ready(
        &mut self,
        amount_deliverable: Amount,
        timestamp: DateTime<Utc>,
        zero_threshold: Amount,
    ) -> Option<()> {
        let res = self
            .open_lots
            .iter()
            .fold_while(Some((amount_deliverable, 0)), |acc, lot| {
                let res = acc.and_then(|(amount, pos)| {
                    let remaining_amount = safe!(amount - lot.unconfirmed_amount)?;
                    if -zero_threshold < remaining_amount {
                        Some(Continue(Some((remaining_amount, pos + 1))))
                    } else {
                        Some(Done(Some((amount, pos))))
                    }
                });
                match res {
                    Some(res) => res,
                    None => Done(None),
                }
            });

        let (amount, pos) = res.into_inner()?;
        if res.is_done() {
            let lot = &mut self.open_lots[pos];
            let ready_balance = safe!(lot.ready_amount + amount)?;
            let unconfirmed_balance = safe!(lot.unconfirmed_amount - amount)?;
            lot.unconfirmed_amount = unconfirmed_balance;
            lot.ready_amount = ready_balance;
            println!(
                "(colateral-side) AddReady for {} {:0.5} ua={:0.5} ra={:0.5} pa={:0.5} sa={:0.5} (partial confirm)",
                lot.payment_id, amount, lot.unconfirmed_amount, lot.ready_amount, lot.preauth_amount, lot.spent_amount
            );
        }

        for lot in self.open_lots.iter_mut().take(pos) {
            let amount = lot.unconfirmed_amount;
            let ready_balance = safe!(lot.ready_amount + amount)?;
            lot.unconfirmed_amount = Amount::ZERO;
            lot.ready_amount = ready_balance;
            lot.last_update_timestamp = timestamp;
            println!(
                "(colateral-side) AddReady for {} {:0.5} ua={:0.5} ra={:0.5} pa={:0.5} sa={:0.5} (full confirm)",
                lot.payment_id, amount, lot.unconfirmed_amount, lot.ready_amount, lot.preauth_amount, lot.spent_amount
            );
        }

        let ready_balance = safe!(self.ready_balance + amount_deliverable)?;
        let unconfirmed_balance = safe!(self.unconfirmed_balance - amount_deliverable)?;

        self.ready_balance = ready_balance;
        self.unconfirmed_balance = unconfirmed_balance;
        self.last_update_timestamp = timestamp;

        Some(())
    }

    pub fn preauth_payment(
        &mut self,
        client_order_id: &ClientOrderId,
        payment_id: PaymentId,
        timestamp: DateTime<Utc>,
        amount_payable: Amount,
        zero_threshold: Amount,
    ) -> Option<PreAuthStatus> {
        let res = self
            .open_lots
            .iter()
            .fold_while(Some((amount_payable, 0)), |acc, lot| {
                let res = acc.and_then(|(amount, pos)| {
                    let remaining_amount = safe!(amount - lot.ready_amount)?;
                    if -zero_threshold < remaining_amount {
                        Some(Continue(Some((remaining_amount, pos + 1))))
                    } else {
                        Some(Done(Some((amount, pos))))
                    }
                });
                match res {
                    Some(res) => res,
                    None => Done(None),
                }
            });

        let (amount, pos) = res.into_inner()?;
        if res.is_done() {
            let lot = &mut self.open_lots[pos];
            let preauth_balance = safe!(lot.preauth_amount + amount)?;
            let ready_balance = safe!(lot.ready_amount - amount)?;
            lot.ready_amount = ready_balance;
            lot.preauth_amount = preauth_balance;
            let spend = CollateralSpend {
                client_order_id: client_order_id.clone(),
                payment_id: payment_id.clone(),
                preauth_amount: amount,
                spent_amount: Amount::ZERO,
                timestamp,
            };
            println!(
                "(colateral-side) PreAuth for {} [{}] {} {:0.5} ua={:0.5} ra={:0.5} pa={:0.5} sa={:0.5} (partial preauth)",
                lot.payment_id, spend.payment_id, spend.client_order_id, spend.preauth_amount,
                lot.unconfirmed_amount, lot.ready_amount, lot.preauth_amount, lot.spent_amount
            );
            lot.spends.push(spend);
        }

        for lot in self.open_lots.iter_mut().take(pos) {
            let amount = lot.ready_amount;
            let preauth_balance = safe!(lot.preauth_amount + amount)?;
            lot.ready_amount = Amount::ZERO;
            lot.preauth_amount = preauth_balance;
            lot.last_update_timestamp = timestamp;
            let spend = CollateralSpend {
                client_order_id: client_order_id.clone(),
                payment_id: payment_id.clone(),
                preauth_amount: amount,
                spent_amount: Amount::ZERO,
                timestamp,
            };
            println!(
                "(colateral-side) PreAuth for {} [{}] {} {:0.5} ua={:0.5} ra={:0.5} pa={:0.5} sa={:0.5} (full preauth)",
                lot.payment_id, spend.payment_id, spend.client_order_id, spend.preauth_amount,
                lot.unconfirmed_amount, lot.ready_amount, lot.preauth_amount, lot.spent_amount
            );
            lot.spends.push(spend);
        }

        let ready_balance = safe!(self.ready_balance - amount_payable)?;
        let preauth_balance = safe!(self.preauth_balance + amount_payable)?;

        self.ready_balance = ready_balance;
        self.preauth_balance = preauth_balance;

        Some(PreAuthStatus::Approved { payment_id })
    }

    pub fn confirm_payment(
        &mut self,
        payment_id: &PaymentId,
        timestamp: DateTime<Utc>,
        amount_paid: Amount,
        zero_threshold: Amount,
    ) -> Option<ConfirmStatus> {
        let res = self
            .open_lots
            .iter()
            .fold_while(Some((amount_paid, 0)), |acc, lot| {
                let res = acc.and_then(|(amount, pos)| {
                    // We answer the question:
                    // How much preauth for this particular payment ID is in this lot?
                    let lot_preauth_amount: Amount = lot
                        .spends
                        .iter()
                        .filter(|spend| spend.payment_id.eq(payment_id))
                        .map(|spend| spend.preauth_amount)
                        .sum();
                    // We just want to find boundary position after which we
                    // don't want to scan lots
                    let remaining_amount = safe!(amount - lot_preauth_amount)?;
                    if -zero_threshold < remaining_amount {
                        Some(Continue(Some((remaining_amount, pos + 1))))
                    } else {
                        Some(Done(Some((amount, pos))))
                    }
                });
                match res {
                    Some(res) => res,
                    None => Done(None),
                }
            });

        let (amount, pos) = res.into_inner()?;

        let mut closed_lots = Vec::new();

        // Fully close spends (not lots!)
        for lot in self.open_lots.iter_mut().take(pos) {
            // Update spends
            let mut spent_amount = Amount::ZERO;
            for spend in lot
                .spends
                .iter_mut()
                .filter(|spend| spend.payment_id.eq(payment_id))
            {
                let amount = spend.preauth_amount;
                let spent_balance = safe!(spend.spent_amount + amount)?;
                spend.preauth_amount = Amount::ZERO;
                spend.spent_amount = spent_balance;
                spend.timestamp = timestamp;
                // Total amount spent in spends for this payment ID in this lot
                spent_amount = safe!(spent_amount + amount)?;
            }

            let amount = spent_amount;
            let spent_balance = safe!(lot.spent_amount + amount)?;
            lot.preauth_amount = safe!(lot.preauth_amount - amount)?;
            lot.spent_amount = spent_balance;
            lot.last_update_timestamp = timestamp;

            if lot.unconfirmed_amount < zero_threshold
                && lot.ready_amount < zero_threshold
                && lot.preauth_amount < zero_threshold
            {
                println!(
                    "(colateral-side) Spend for {} [{}] {:0.5} ua={:0.5} ra={:0.5} pa={:0.5} sa={:0.5} (full spend)",
                        lot.payment_id, payment_id, amount,
                        lot.unconfirmed_amount, lot.ready_amount, lot.preauth_amount, lot.spent_amount
                    );

                closed_lots.push(lot.payment_id.clone());
            } else {
                println!(
                    "(colateral-side) Spend for {} [{}] {:0.5} ua={:0.5} ra={:0.5} pa={:0.5} sa={:0.5} (partial spend)",
                        lot.payment_id, payment_id, amount,
                        lot.unconfirmed_amount, lot.ready_amount, lot.preauth_amount, lot.spent_amount
                    );
            }
        }

        if res.is_done() {
            let lot = &mut self.open_lots[pos];

            // Update spends
            let mut spent_amount = Amount::ZERO;
            for spend in lot
                .spends
                .iter_mut()
                .filter(|spend| spend.payment_id.eq(payment_id))
            {
                let amount = safe!(amount - spent_amount)?;
                let amount_remaining = safe!(amount - spend.preauth_amount)?;
                if zero_threshold < amount_remaining {
                    let sub_amount = spend.preauth_amount;
                    let spent_balance = safe!(spend.spent_amount + sub_amount)?;
                    spend.preauth_amount = Amount::ZERO;
                    spend.spent_amount = spent_balance;
                    spend.timestamp = timestamp;
                    // Total amount spent in spends for this payment ID in this lot
                    spent_amount = safe!(spent_amount + sub_amount)?;
                } else {
                    let spent_balance = safe!(spend.spent_amount + amount)?;
                    spend.preauth_amount = Amount::ZERO;
                    spend.spent_amount = spent_balance;
                    spend.timestamp = timestamp;
                    // Total amount spent in spends for this payment ID in this lot
                    spent_amount = safe!(spent_amount + amount)?;
                }
            }

            let spent_balance = safe!(lot.spent_amount + amount)?;
            let preauth_balance = safe!(lot.preauth_amount - amount)?;
            lot.preauth_amount = preauth_balance;
            lot.spent_amount = spent_balance;

            println!(
                "(colateral-side) Spend for {} [{}] {:0.5} ua={:0.5} ra={:0.5} pa={:0.5} sa={:0.5} (partial spend)",
                lot.payment_id, payment_id, amount,
                lot.unconfirmed_amount, lot.ready_amount, lot.preauth_amount, lot.spent_amount
            );
        }

        //self.open_lots.splice(range, replace_with)

        let preauth_balance = safe!(self.preauth_balance - amount_paid)?;
        let spent_balance = safe!(self.spent_balance + amount_paid)?;

        self.preauth_balance = preauth_balance;
        self.spent_balance = spent_balance;

        Some(ConfirmStatus::Authorized)
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
            side_cr: CollateralSide::new(timestmap),
            side_dr: CollateralSide::new(timestmap),
            created_timestamp: timestmap,
            last_update_timestamp: timestmap,
        }
    }

    pub fn add_ready(
        &mut self,
        side: Side,
        amount: Amount,
        timestamp: DateTime<Utc>,
        zero_threshold: Amount,
    ) -> Option<()> {
        // Swap cr/dr for Sell - maybe it will work :)
        let side_cr = match side {
            Side::Buy => &mut self.side_cr,
            Side::Sell => &mut self.side_dr,
        };

        side_cr.add_ready(amount, timestamp, zero_threshold)
    }

    pub fn preauth_payment(
        &mut self,
        client_order_id: &ClientOrderId,
        timestamp: DateTime<Utc>,
        side: Side,
        amount_payable: Amount,
        zero_threshold: Amount,
        next_payment_id: impl Fn() -> PaymentId,
    ) -> Option<PreAuthStatus> {
        // Swap cr/dr for Sell - maybe it will work :)
        let (side_cr, side_dr) = match side {
            Side::Buy => (&mut self.side_cr, &mut self.side_dr),
            Side::Sell => (&mut self.side_dr, &mut self.side_cr),
        };

        let balance_remaining = safe!(side_cr.ready_balance - amount_payable)?;

        if balance_remaining < -zero_threshold {
            Some(PreAuthStatus::NotEnoughFunds)
        } else {
            side_cr.preauth_payment(
                client_order_id,
                next_payment_id(),
                timestamp,
                amount_payable,
                zero_threshold,
            )
        }
    }

    pub fn confirm_payment(
        &mut self,
        payment_id: &PaymentId,
        timestamp: DateTime<Utc>,
        side: Side,
        amount_paid: Amount,
        zero_threshold: Amount,
    ) -> Option<ConfirmStatus> {
        // Swap cr/dr for Sell - maybe it will work :)
        let side_cr = match side {
            Side::Buy => &mut self.side_cr,
            Side::Sell => &mut self.side_dr,
        };
        let balance_remaining = safe!(side_cr.preauth_balance - amount_paid)?;

        if balance_remaining < -zero_threshold {
            Some(ConfirmStatus::NotEnoughFunds)
        } else {
            side_cr.confirm_payment(payment_id, timestamp, amount_paid, zero_threshold)
        }
    }
}

pub trait CollateralManagerHost: SetSolverOrderStatus {
    fn get_next_payment_id(&self) -> PaymentId;
}

pub struct CollateralManager {
    observer: SingleObserver<CollateralEvent>,
    router: Arc<ComponentLock<CollateralRouter>>,
    client_funds: HashMap<(u32, Address), Arc<RwLock<CollateralPosition>>>,
    collateral_management_requests: VecDeque<CollateralManagement>,
    zero_threshold: Amount,
}

impl CollateralManager {
    pub fn new(router: Arc<ComponentLock<CollateralRouter>>, zero_threshold: Amount) -> Self {
        Self {
            observer: SingleObserver::new(),
            router,
            client_funds: HashMap::new(),
            collateral_management_requests: VecDeque::new(),
            zero_threshold,
        }
    }

    pub fn process_collateral(
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

        let failures = ready_to_route
            .into_iter()
            .filter_map(|(request, unconfirmed_balance)| {
                match self
                    .router
                    .write()
                    .map_err(|e| eyre!("Failed to access router {}", e))
                    .and_then(|x| {
                        x.transfer_collateral(
                            request.chain_id,
                            request.address,
                            request.client_order_id.clone(),
                            request.side,
                            unconfirmed_balance,
                        )
                    }) {
                    Ok(()) => None,
                    Err(err) => Some((err, request)),
                }
            })
            .collect_vec();

        if !failures.is_empty() {
            eprintln!(
                "(collateral-manager) Errors in processing: {}",
                failures
                    .into_iter()
                    .map(|(err, request)| {
                        format!(
                            "\n in {} {} {}: {:?}",
                            request.chain_id,
                            request.address,
                            request.client_order_id.clone(),
                            err
                        )
                    })
                    .join(", ")
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

        collateral_position_write.side_cr.open_lot(
            host.get_next_payment_id(),
            amount,
            timestamp,
        )?;

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

        collateral_position_write.side_dr.open_lot(
            host.get_next_payment_id(),
            amount,
            timestamp,
        )?;

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
        address: &Address,
        client_order_id: &ClientOrderId,
        timestamp: DateTime<Utc>,
        side: Side,
        amount_payable: Amount,
    ) -> Result<()> {
        println!(
            "(collateral-manager) PreAuth Payment for {} {:0.5}",
            address, amount_payable
        );

        let funds = self
            .get_position(chain_id, &address)
            .ok_or_eyre("Failed to find position")?;

        let status = funds
            .write()
            .preauth_payment(
                &client_order_id,
                timestamp,
                side,
                amount_payable,
                self.zero_threshold,
                || host.get_next_payment_id(),
            )
            .ok_or_eyre("Math Problem")?;

        self.observer
            .publish_single(CollateralEvent::PreAuthResponse {
                chain_id,
                address: *address,
                client_order_id: client_order_id.clone(),
                amount_payable,
                timestamp,
                status,
            });

        Ok(())
    }

    /// Confirm Payment
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
        timestamp: DateTime<Utc>,
        side: Side,
        amount_paid: Amount,
    ) -> Result<()> {
        println!(
            "(collateral-manager) Confirm Payment for {} {:0.5}",
            address, amount_paid
        );

        let funds = self
            .get_position(chain_id, &address)
            .ok_or_eyre("Failed to find position")?;

        let status = funds
            .write()
            .confirm_payment(
                payment_id,
                timestamp,
                side,
                amount_paid,
                self.zero_threshold,
            )
            .ok_or_eyre("Math Problem")?;

        self.observer
            .publish_single(CollateralEvent::ConfirmResponse {
                chain_id,
                address: *address,
                client_order_id: client_order_id.clone(),
                payment_id: payment_id.clone(),
                amount_paid,
                timestamp,
                status,
            });

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
                timestamp,
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

                let mut funds_write = funds.write();

                // TODO: We need to also support Sell & Withdraw
                let side = Side::Buy;

                funds_write
                    .add_ready(side, amount, timestamp, self.zero_threshold)
                    .ok_or_eyre("Math Problem")?;

                // TODO: Charge fee otherwise we'll have dangling unconfirmed amount
                let get_payment_id = || "Charges".into();
                let payment_id = get_payment_id();
                funds_write
                    .add_ready(side, fee, timestamp, self.zero_threshold)
                    .ok_or_eyre("Math Problem")?;
                funds_write
                    .preauth_payment(
                        &client_order_id,
                        timestamp,
                        side,
                        fee,
                        self.zero_threshold,
                        get_payment_id,
                    )
                    .ok_or_eyre("Math Problem")?;
                //funds_write
                //    .preauth_payment(
                //        &client_order_id,
                //        timestamp,
                //        side,
                //        amount,
                //        self.zero_threshold,
                //        get_payment_id,
                //    )
                //    .ok_or_eyre("Math Problem")?;
                funds_write
                    .confirm_payment(&payment_id, timestamp, side, fee, self.zero_threshold)
                    .ok_or_eyre("Math Problem")?;

                self.observer
                    .publish_single(CollateralEvent::CollateralReady {
                        chain_id,
                        address,
                        client_order_id,
                        timestamp,
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
