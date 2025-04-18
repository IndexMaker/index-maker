use std::{collections::VecDeque, fmt::Display, sync::Arc};

use chrono::{DateTime, Utc};
use eyre::{eyre, Result};
use parking_lot::RwLock;

use crate::core::{
    bits::{Amount, BatchOrderId, OrderId, Side, Symbol},
    functional::PublishSingle,
};

use super::inventory_manager::InventoryEvent;

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
#[derive(Default, Hash, Eq, PartialEq, Clone, Debug)]
pub struct LotId(pub String);

impl Display for LotId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "ClientOrderID({})", self.0)
    }
}

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
    pub lot_transactions: Vec<LotTransaction>,
}
pub struct Position {
    /// An asset we received
    pub symbol: Symbol,

    /// Buy (Long) or Sell (Short)
    pub side: Side,

    /// Balance (>0 if Long, <0 if Short)
    pub balance: Amount,

    /// Lots open on the side
    ///
    /// We need to store Lots in Arc, because they are too expensive to copy or move.
    /// We also need to store them in RwLock, because we will be updating them. We're
    /// using cheap parking_lot RwLock, and our updates will not be holding lock for
    /// long, just write few fields and unlock.
    pub open_lots: VecDeque<Arc<RwLock<Lot>>>,

    /// Lots closed by lot_transactions
    pub closed_lots: VecDeque<Arc<RwLock<Lot>>>,
}

impl Position {
    pub fn new(symbol: Symbol, side: Side) -> Self {
        Self {
            symbol,
            side,
            balance: Amount::ZERO,
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
        original_quantity: Amount,
        quantity_remaining: Amount,
        fill_timestamp: DateTime<Utc>,
        observer: &impl PublishSingle<InventoryEvent>,
    ) -> Result<()> {
        // Store as open lot
        self.open_lots.push_back(Arc::new(RwLock::new(Lot {
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
        })));
        // Update balance
        self.balance = match side {
            Side::Buy => self.balance.checked_add(quantity_filled),
            Side::Sell => self.balance.checked_sub(quantity_filled),
        }
        .ok_or(eyre!("Math overflow"))?;

        observer.publish_single(InventoryEvent::OpenLot {
            order_id,
            batch_order_id,
            lot_id,
            symbol: self.symbol.clone(),
            side,
            price: price_filled,
            quantity: quantity_filled,
            fee: fee_paid,
            original_batch_quantity: original_quantity,
            batch_quantity_remaining: quantity_remaining,
            timestamp: fill_timestamp,
        });
        Ok(())
    }

    pub fn match_lots(
        &mut self,
        order_id: OrderId,
        batch_order_id: BatchOrderId,
        lot_id: LotId,
        symbol: Symbol,
        side: Side,
        price_filled: Amount,
        mut quantity_filled: Amount,
        fee_paid: Amount,
        batch_original_quantity: Amount,
        batch_quantity_remaining: Amount,
        fill_timestamp: DateTime<Utc>,
        observer: &impl PublishSingle<InventoryEvent>,
        tolerance: Amount,
    ) -> Result<Option<Amount>> {
        // Match only if opposite side
        if self.side == side {
            return Ok(Some(quantity_filled));
        }
        while let Some(lot) = self.open_lots.front().cloned() {
            let lot_quantity_remaining = lot.read().remaining_quantity;

            let remaining_quantity = lot_quantity_remaining
                .checked_sub(quantity_filled)
                .ok_or(eyre!("Math overflow"))?;

            let (matched_lot_quantity, lot_quantity_remaining, finished) =
                if remaining_quantity < tolerance {
                    // We closed the lotd
                    let lot = self.open_lots.pop_front().unwrap();
                    // ...we move it to closed_lots
                    self.closed_lots.push_back(lot);
                    (
                        // we matched whole lot
                        lot_quantity_remaining,
                        // there is no quantity remaining on the lot
                        Amount::ZERO,
                        // check if we should continue
                        if remaining_quantity < -tolerance {
                            // there is some quantity left to continue matching
                            quantity_filled = -remaining_quantity;
                            println!(
                                "there is some quantity left to continue matching {}",
                                quantity_filled
                            );
                            false
                        } else {
                            // fill was fully matched
                            quantity_filled = Amount::ZERO;
                            true
                        },
                    )
                } else {
                    println!("We matched whole quantity of fill {}", quantity_filled);
                    // We matched whole quantity of fill
                    let matched_lot_quantity = quantity_filled;

                    // fill was fully matched
                    quantity_filled = Amount::ZERO;

                    // We partly closed the lot
                    (matched_lot_quantity, remaining_quantity, true)
                };

            // Update lot
            // We do it regardless whether it is still open or closed, as we
            // have an Arc reference to it.
            {
                let mut lot = lot.write();

                lot.lot_transactions.push(LotTransaction {
                    order_id: order_id.clone(),
                    batch_order_id: batch_order_id.clone(),
                    matched_lot_id: lot_id.clone(),
                    closing_fee: fee_paid,
                    closing_price: price_filled,
                    closing_timestamp: fill_timestamp,
                    quantity_closed: matched_lot_quantity,
                });

                lot.last_update_timestamp = fill_timestamp;
                lot.remaining_quantity = lot_quantity_remaining;

                observer.publish_single(InventoryEvent::CloseLot {
                    original_order_id: lot.original_order_id.clone(),
                    original_batch_order_id: lot.original_batch_order_id.clone(),
                    original_lot_id: lot.lot_id.clone(),
                    closing_order_id: order_id.clone(),
                    closing_batch_order_id: batch_order_id.clone(),
                    closing_lot_id: lot_id.clone(),
                    symbol: symbol.clone(),
                    side: self.side,
                    original_price: lot.original_price,
                    closing_price: price_filled,
                    closing_fee: fee_paid,
                    quantity_closed: matched_lot_quantity,
                    original_quantity: lot.original_quantity,
                    quantity_remaining: lot_quantity_remaining,
                    original_timestamp: lot.created_timestamp,
                    closing_batch_original_quantity: batch_original_quantity,
                    closing_batch_quantity_remaining: batch_quantity_remaining,
                    closing_timestamp: fill_timestamp,
                });
            }

            if finished {
                break;
            }
        }

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
}
