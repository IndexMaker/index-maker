use std::collections::VecDeque;

use chrono::{DateTime, Utc};
use itertools::FoldWhile::{Continue, Done};
use itertools::Itertools;

use eyre::{OptionExt, Result};
use safe_math::safe;

use symm_core::core::{
    bits::{Address, Amount, ClientOrderId, PaymentId, Side},
    decimal_ext::DecimalExt,
};

pub enum PreAuthStatus {
    Approved { payment_id: PaymentId },
    NotEnoughFunds,
}
pub enum ConfirmStatus {
    Authorized,
    NotEnoughFunds,
}

pub struct CollateralSpend {
    /// Client Order ID
    pub client_order_id: ClientOrderId,

    /// Payment ID of this spend
    pub payment_id: PaymentId,

    /// Amount of funding (pre-authorized)
    pub preauth_amount: Amount,

    /// Amount spent
    pub spent_amount: Amount,

    /// Time of spend
    pub timestamp: DateTime<Utc>,
}

pub struct CollateralLot {
    /// Payment ID of this funding (credit/debit)
    pub payment_id: PaymentId,

    /// Total amount of funding (unconfirmed)
    pub unconfirmed_amount: Amount,

    /// Total amount of funding (ready to trade)
    pub ready_amount: Amount,

    /// Total amount of funding (pre-authorized)
    pub preauth_amount: Amount,

    /// Total amount spent (spent on trade)
    pub spent_amount: Amount,

    /// Time of funding
    pub created_timestamp: DateTime<Utc>,

    /// Time of the last update
    pub last_update_timestamp: DateTime<Utc>,

    /// Collateral spent for orders
    pub spends: Vec<CollateralSpend>,
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

pub struct CollateralSide {
    /// Total amount of funding (unconfirmed)
    pub unconfirmed_balance: Amount,

    /// Total amount of funding (ready to trade)
    pub ready_balance: Amount,

    /// Total amount of funding (pre-authorized)
    pub preauth_balance: Amount,

    /// Total amount spent (spent on trade)
    pub spent_balance: Amount,

    /// Lots open w/ some non-zero balance (unconfirmed + ready)
    pub open_lots: Vec<CollateralLot>,

    /// Lots closed w/ all balance spent
    pub closed_lots: VecDeque<CollateralLot>,

    /// Time position was created
    pub created_timestamp: DateTime<Utc>,

    /// Last time we updated this position
    pub last_update_timestamp: DateTime<Utc>,
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
            tracing::info!(
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
            tracing::info!(
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
            tracing::info!(
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
            tracing::info!(
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

        let mut closed_lots = VecDeque::new();

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
                tracing::info!(
                    "(colateral-side) Spend for {} [{}] {:0.5} ua={:0.5} ra={:0.5} pa={:0.5} sa={:0.5} (full spend)",
                        lot.payment_id, payment_id, amount,
                        lot.unconfirmed_amount, lot.ready_amount, lot.preauth_amount, lot.spent_amount
                    );

                closed_lots.push_back(lot.payment_id.clone());
            } else {
                tracing::info!(
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

            tracing::info!(
                "(colateral-side) Spend for {} [{}] {:0.5} ua={:0.5} ra={:0.5} pa={:0.5} sa={:0.5} (partial spend *)",
                lot.payment_id, payment_id, amount,
                lot.unconfirmed_amount, lot.ready_amount, lot.preauth_amount, lot.spent_amount
            );
        }

        for lot_id in closed_lots {
            let (pos, _) = self
                .open_lots
                .iter()
                .find_position(|x| lot_id.eq(&x.payment_id))?;
            let lot = self.open_lots.remove(pos);
            self.closed_lots.push_back(lot);
        }

        let preauth_balance = safe!(self.preauth_balance - amount_paid)?;
        let spent_balance = safe!(self.spent_balance + amount_paid)?;

        self.preauth_balance = preauth_balance;
        self.spent_balance = spent_balance;

        Some(ConfirmStatus::Authorized)
    }
}

pub struct CollateralPosition {
    /// Chain ID
    pub chain_id: u32,

    /// On-chain address of the User
    pub address: Address,

    /// Credits
    pub side_cr: CollateralSide,

    /// Debits
    pub side_dr: CollateralSide,

    /// Time position was created
    pub created_timestamp: DateTime<Utc>,

    /// Last time we updated this position
    pub last_update_timestamp: DateTime<Utc>,
}

impl CollateralPosition {
    pub fn new(chain_id: u32, address: Address, timestamp: DateTime<Utc>) -> Self {
        Self {
            chain_id,
            address,
            side_cr: CollateralSide::new(timestamp),
            side_dr: CollateralSide::new(timestamp),
            created_timestamp: timestamp,
            last_update_timestamp: timestamp,
        }
    }

    pub fn deposit(
        &mut self,
        payment_id: PaymentId,
        amount: Amount,
        timestamp: DateTime<Utc>,
    ) -> Result<()> {
        self.side_cr.open_lot(payment_id, amount, timestamp)?;
        self.last_update_timestamp = timestamp;
        Ok(())
    }

    pub fn withdraw(
        &mut self,
        payment_id: PaymentId,
        amount: Amount,
        timestamp: DateTime<Utc>,
    ) -> Result<()> {
        self.side_dr.open_lot(payment_id, amount, timestamp)?;
        self.last_update_timestamp = timestamp;
        Ok(())
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
        let side_cr = match side {
            Side::Buy => &mut self.side_cr,
            Side::Sell => &mut self.side_dr,
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

#[cfg(test)]
mod test {
    use chrono::Utc;
    use rust_decimal::dec;
    use test_case::test_case;

    use symm_core::{
        assert_decimal_approx_eq,
        core::{
            bits::{Amount, Side},
            test_util::get_mock_address_1,
        },
    };

    use crate::collateral::collateral_position::{ConfirmStatus, PreAuthStatus};

    use super::CollateralPosition;

    /// Test Collateral Position
    /// ---
    /// Collateral Position maintains single account of user's collateral.  It
    /// tracks what happened with each collateral from deposit, through routing,
    /// to spending on minting. Collateral position tracks the balances and lots
    /// associated with those stages of collateral movement.
    ///
    #[test_case(
        (dec!(1000.0), dec!(900.0), dec!(900.0), dec!(500.0)),
        (dec!(1000.0), dec!(0.0), dec!(0.0), dec!(0.0)),
        (dec!(100.0), dec!(900.0), dec!(0.0), dec!(0.0)),
        (dec!(100.0), dec!(0.0), dec!(900.0), dec!(0.0)),
        (dec!(100.0), dec!(0.0), dec!(400.0), dec!(500.0)),
        false; "partial ready"
    )]
    #[test_case(
        (dec!(1000.0), dec!(1000.0), dec!(900.0), dec!(500.0)),
        (dec!(1000.0), dec!(0.0), dec!(0.0), dec!(0.0)),
        (dec!(0.0), dec!(1000.0), dec!(0.0), dec!(0.0)),
        (dec!(0.0), dec!(100.0), dec!(900.0), dec!(0.0)),
        (dec!(0.0), dec!(100.0), dec!(400.0), dec!(500.0)),
        false; "partial preauth"
    )]
    #[test_case(
        (dec!(1000.0), dec!(1000.0), dec!(1000.0), dec!(500.0)),
        (dec!(1000.0), dec!(0.0), dec!(0.0), dec!(0.0)),
        (dec!(0.0), dec!(1000.0), dec!(0.0), dec!(0.0)),
        (dec!(0.0), dec!(0.0), dec!(1000.0), dec!(0.0)),
        (dec!(0.0), dec!(0.0), dec!(500.0), dec!(500.0)),
        false; "partial spend"
    )]
    #[test_case(
        (dec!(1000.0), dec!(900.0), dec!(800.0), dec!(500.0)),
        (dec!(1000.0), dec!(0.0), dec!(0.0), dec!(0.0)),
        (dec!(100.0), dec!(900.0), dec!(0.0), dec!(0.0)),
        (dec!(100.0), dec!(100.0), dec!(800.0), dec!(0.0)),
        (dec!(100.0), dec!(100.0), dec!(300.0), dec!(500.0)),
        false; "partial all"
    )]
    #[test_case(
        (dec!(1000.0), dec!(1000.0), dec!(1000.0), dec!(1000.0)),
        (dec!(1000.0), dec!(0.0), dec!(0.0), dec!(0.0)),
        (dec!(0.0), dec!(1000.0), dec!(0.0), dec!(0.0)),
        (dec!(0.0), dec!(0.0), dec!(1000.0), dec!(0.0)),
        (dec!(0.0), dec!(0.0), dec!(0.0), dec!(1000.0)),
        true; "full spend"
    )]
    fn test_collateral_position(
        inputs: (Amount, Amount, Amount, Amount),
        post_deposit: (Amount, Amount, Amount, Amount),
        post_ready: (Amount, Amount, Amount, Amount),
        post_preauth: (Amount, Amount, Amount, Amount),
        post_confirm: (Amount, Amount, Amount, Amount),
        full_spend: bool,
    ) {
        let timestamp = Utc::now();
        let zero_threshold = dec!(0.0001);

        let (deposit, ready, preauth, confirm) = inputs;

        let mut pos = CollateralPosition::new(1, get_mock_address_1(), timestamp);

        let test_asserts = |pos: &CollateralPosition, expected, check_closed_lot| {
            let (unconfirmed, ready, preauth, confirm) = expected;

            let side = &pos.side_cr;

            assert_decimal_approx_eq!(side.unconfirmed_balance, unconfirmed, zero_threshold);
            assert_decimal_approx_eq!(side.ready_balance, ready, zero_threshold);
            assert_decimal_approx_eq!(side.preauth_balance, preauth, zero_threshold);
            assert_decimal_approx_eq!(side.spent_balance, confirm, zero_threshold);

            let first_lot = if check_closed_lot {
                assert!(pos.side_cr.open_lots.is_empty());
                assert_eq!(pos.side_cr.closed_lots.len(), 1);
                &pos.side_cr.closed_lots[0]
            } else {
                assert!(pos.side_cr.closed_lots.is_empty());
                assert_eq!(pos.side_cr.open_lots.len(), 1);
                &pos.side_cr.open_lots[0]
            };

            assert_decimal_approx_eq!(first_lot.unconfirmed_amount, unconfirmed, zero_threshold);
            assert_decimal_approx_eq!(first_lot.ready_amount, ready, zero_threshold);
            assert_decimal_approx_eq!(first_lot.preauth_amount, preauth, zero_threshold);
            assert_decimal_approx_eq!(first_lot.spent_amount, confirm, zero_threshold);
        };

        // User deposits
        // -------------
        // This operation creates unconfirmed lots of collateral. Normally routing
        // would perform transfer from source chain into final destination, and
        // once transfers are complete, then we move to next state, which is ready.
        pos.deposit("P-01".into(), deposit, timestamp).unwrap();

        test_asserts(&pos, post_deposit, false);

        // Ready after collateral routing
        // ------------------------------
        // This operation selects unconfirmed lots of collateral, and moves them
        // into ready state. This normally means collateral arrived at destination
        // and we are recording this fact by moving its lots to ready state.
        pos.add_ready(Side::Buy, ready, timestamp, zero_threshold)
            .unwrap();

        test_asserts(&pos, post_ready, false);

        // Solver takes ownership of ready collateral
        // ------------------------------------------
        // This operation selects the lots of ready collateral, and moves them
        // into preauth state returning Payment ID to be used for this client
        // order. This is normally performed by Solver, once it learns that
        // collateral is ready.
        let status = pos
            .preauth_payment(
                &"C-01".into(),
                timestamp,
                Side::Buy,
                preauth,
                zero_threshold,
                || "P-02".into(),
            )
            .unwrap();

        assert!(matches!(status, PreAuthStatus::Approved { .. }));

        test_asserts(&pos, post_preauth, false);

        // Solver confirms the payment just before minting index token
        // -----------------------------------------------------------
        // This operation selects preauthorized lots and moves them
        // into spent state. This is normally performed by Sover, once
        // index order becomes mintable, and Solver wants to mint the token.
        let status = pos
            .confirm_payment(
                &"P-02".into(),
                timestamp,
                Side::Buy,
                confirm,
                zero_threshold,
            )
            .unwrap();

        assert!(matches!(status, ConfirmStatus::Authorized));

        test_asserts(&pos, post_confirm, full_spend);
    }
}
