use std::{collections::VecDeque, sync::Arc};

use chrono::{DateTime, Utc};
use eyre::{ensure, eyre, OptionExt, Result};
use safe_math::safe;
use serde::{Deserialize, Serialize};

use crate::{
    core::{
        bits::{Amount, BatchOrderId, OrderId, Side, Symbol},
        decimal_ext::DecimalExt,
    },
    string_id,
};

string_id!(LotId);

#[derive(Serialize, Deserialize, Debug)]
pub struct LotTransaction {
    /// ID of the closing order that was executed
    pub order_id: OrderId,

    /// ID of the associated batch order
    pub batch_order_id: BatchOrderId,

    /// ID of the matching lot, essentially ID of the transaction, which closed portion of the lot
    pub matched_lot_id: LotId,

    /// Quantity from matching transaction, it can be same as quantity on the transaction or less,
    /// because matching transaction could have more quantity than available in this lot, and had
    /// to be matched against more than one lot.
    pub quantity_closed: Amount,

    /// Price on the matching transaction
    pub closing_price: Amount,

    /// Fee paid
    pub closing_fee: Amount,

    /// Time of the closing transaction
    pub closing_timestamp: DateTime<Utc>,
}

/// Lot is what you get in a single execution, so Lot Id is same as execution Id and comes from exchange (<- Binance)
///
/// From exchange perspective execution Id is the Id of the *action*, which is to execute an order.
/// However from our perspective, when our order is executed what we receive is a *lot* of an asset,
/// for us it is not execution that matters, but the actual quantity of asset we received in one
/// transaction, and that we call *lot*. We manage lots and not executions. We handle executions by managing lots.
/// When we get an execution of the Buy order, then we open a lot, and when we get an execution of the Sell order
/// we match that new lot against the one we opened for Buy order. Lots form a stack (LIFO) or queue (FIFO).
/// We always match incoming lot from Sell transation against current stack/queue. Note that we said Buy opens a lot
/// and Sell closes one or more lots. When short-selling is supported these can be inverted, but we don't support
/// short-selling.
///

#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct Lot {
    /// ID of the order that was executed, and caused to open this lot
    pub original_order_id: OrderId,

    /// ID of the associated batch order
    pub original_batch_order_id: BatchOrderId,

    /// ID of this lot, essentially ID of the transaction, which opened this lot
    pub lot_id: LotId,

    /// Price on transaction that opened this lot
    pub original_price: Amount,

    /// Quantity on transaction that opened this lot
    pub original_quantity: Amount,

    /// Fee paid
    pub original_fee: Amount,

    /// Quantity remaining in this lot after most recent transation
    pub remaining_quantity: Amount,

    /// Time of the first transaction that opened this lot
    pub created_timestamp: DateTime<Utc>,

    /// Time of the last transaction matched against this lot
    pub last_update_timestamp: DateTime<Utc>,

    /// All the transactions that were matched against this lot, and these transactions
    /// closed some portion of this lot.
    pub lot_transactions: Vec<Arc<LotTransaction>>,
}

impl Lot {
    pub fn get_last_transaction_quantity(&self) -> Amount {
        self.lot_transactions
            .last()
            .map(|t| t.quantity_closed)
            .unwrap_or_default()
    }
}

#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct Position {
    /// An asset we received
    pub symbol: Symbol,

    /// Buy (Long) or Sell (Short)
    pub side: Side,

    /// Balance (>0 if Long, <0 if Short)
    pub balance: Amount,

    /// Time of the first transaction that opened this lot
    pub created_timestamp: Option<DateTime<Utc>>,

    /// Time of the last transaction matched against this lot
    pub last_update_timestamp: Option<DateTime<Utc>>,

    /// Lots open on the side
    ///
    /// We need to store Lots in Arc, because they are too expensive to copy or move.
    /// We also need to store them in RwLock, because we will be updating them. We're
    /// using cheap parking_lot RwLock, and our updates will not be holding lock for
    /// long, just write few fields and unlock.
    pub open_lots: VecDeque<Box<Lot>>,

    /// Lots closed by lot_transactions
    pub closed_lots: VecDeque<Box<Lot>>,
}

impl Position {
    pub fn new(symbol: Symbol, side: Side) -> Self {
        Self {
            symbol,
            side,
            balance: Amount::ZERO,
            created_timestamp: None,
            last_update_timestamp: None,
            open_lots: VecDeque::new(),
            closed_lots: VecDeque::new(),
        }
    }
    pub fn create_lot(
        &mut self,
        order_id: OrderId,
        batch_order_id: BatchOrderId,
        lot_id: LotId,
        side: Side,
        price_filled: Amount,
        quantity_filled: Amount,
        fee_paid: Amount,
        fill_timestamp: DateTime<Utc>,
    ) -> Result<()> {
        if self.created_timestamp.is_none() {
            self.created_timestamp.replace(fill_timestamp);
        }
        self.last_update_timestamp.replace(fill_timestamp);
        // Store as open lot
        self.open_lots.push_back(Box::new(Lot {
            original_order_id: order_id.clone(),
            original_batch_order_id: batch_order_id.clone(),
            lot_id: lot_id.clone(),
            original_price: price_filled,
            original_quantity: quantity_filled,
            original_fee: fee_paid,
            remaining_quantity: quantity_filled,
            created_timestamp: fill_timestamp,
            last_update_timestamp: fill_timestamp,
            lot_transactions: Vec::new(),
        }));
        // Update balance
        self.balance = match side {
            Side::Buy => safe!(self.balance + quantity_filled),
            Side::Sell => safe!(self.balance + quantity_filled),
        }
        .ok_or(eyre!("Math overflow"))?;
        Ok(())
    }

    pub fn match_lots(
        &mut self,
        order_id: OrderId,
        batch_order_id: BatchOrderId,
        lot_id: LotId,
        side: Side,
        price_filled: Amount,
        mut quantity_filled: Amount,
        fee_paid: Amount,
        fill_timestamp: DateTime<Utc>,
        tolerance: Amount,
    ) -> Result<Option<Amount>> {
        // Match only if opposite side
        if self.side == side {
            return Ok(Some(quantity_filled));
        }
        self.last_update_timestamp.replace(fill_timestamp);
        let mut balance = self.balance;
        let mut front_lot_update = None;
        let mut open_lots_drain_count = 0;
        let mut closed_lots = VecDeque::new();

        for lot in self.open_lots.iter() {
            let lot_quantity_remaining = lot.remaining_quantity;

            let remaining_quantity =
                safe!(lot_quantity_remaining - quantity_filled).ok_or_eyre("Math Problem")?;

            let (matched_lot_quantity, lot_quantity_remaining, finished, was_closed) =
                if remaining_quantity < tolerance {
                    // We closed the lot
                    // ...we move it to closed_lots
                    (
                        // we matched whole lot
                        lot_quantity_remaining,
                        // there is no quantity remaining on the lot
                        Amount::ZERO,
                        // check if we should continue
                        if remaining_quantity < -tolerance {
                            // there is some quantity left to continue matching
                            quantity_filled = -remaining_quantity;
                            tracing::debug!(
                                "there is some quantity left to continue matching {}",
                                quantity_filled
                            );
                            // finished <- false
                            false
                        } else {
                            // fill was fully matched
                            quantity_filled = Amount::ZERO;
                            // finished <- true
                            true
                        },
                        // was_closed <- true
                        true,
                    )
                } else {
                    tracing::debug!("We matched whole quantity of fill {}", quantity_filled);
                    // We matched whole quantity of fill
                    let matched_lot_quantity = quantity_filled;

                    // fill was fully matched
                    quantity_filled = Amount::ZERO;

                    // We partly closed the lot
                    // finished <- true, was_closed <- false
                    (matched_lot_quantity, remaining_quantity, true, false)
                };

            balance = safe!(balance - matched_lot_quantity).ok_or(eyre!("Math overflow"))?;

            let mut lot_mut = lot.clone();

            lot_mut.lot_transactions.push(Arc::new(LotTransaction {
                order_id: order_id.clone(),
                batch_order_id: batch_order_id.clone(),
                matched_lot_id: lot_id.clone(),
                closing_fee: fee_paid,
                closing_price: price_filled,
                closing_timestamp: fill_timestamp,
                quantity_closed: matched_lot_quantity,
            }));

            lot_mut.last_update_timestamp = fill_timestamp;
            lot_mut.remaining_quantity = lot_quantity_remaining;

            if was_closed {
                closed_lots.push_back(lot_mut);
                open_lots_drain_count += 1;
            } else {
                ensure!(finished);
                front_lot_update = Some(lot_mut)
            }

            if finished {
                break;
            }
        }

        // --==| Commit Transaction |==--
        // NOTE: If we failed somewhere in the way, we would keep consistend old state
        let _ = self.open_lots.drain(open_lots_drain_count..).count();
        if let Some(lot) = front_lot_update {
            self.open_lots.pop_front();
            self.open_lots.push_front(lot);
        }
        self.closed_lots = closed_lots;
        self.balance = balance;

        if tolerance < quantity_filled {
            // We didn't fully match this fill against open lots, which means we're going Short, and
            // this should never happen!
            Ok(Some(quantity_filled))
        } else {
            // We fully matched the incoming fill against open lots, and no quantity is remaining
            // Note: This should be expected reusult.
            Ok(None)
        }
    }

    /// Drain and call back on closed lots, and call back on updated lot if any
    pub fn drain_closed_lots_and_callback_on_updated(&mut self, mut cb: impl FnMut(&Lot)) {
        let ref_cb = &mut cb;
        self.closed_lots.drain(..).for_each(|lot| ref_cb(&lot));
        self.open_lots.front().iter().for_each(|lot| {
            if lot.remaining_quantity != lot.original_quantity
                && Some(lot.last_update_timestamp) == self.last_update_timestamp
            {
                ref_cb(&lot);
            }
        });
    }
}
