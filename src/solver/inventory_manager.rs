use std::{
    collections::{hash_map::Entry, HashMap, VecDeque},
    sync::Arc,
};

use chrono::{DateTime, Utc};
use eyre::{eyre, Result};
use itertools::{partition, Itertools};
use parking_lot::RwLock;

use crate::{
    core::{
        bits::{
            Amount, BatchOrder, BatchOrderId, Lot, LotId, LotTransaction, OrderId, Side,
            SingleOrder, Symbol,
        },
        functional::SingleObserver,
    },
    order_sender::order_tracker::{OrderTracker, OrderTrackerNotification},
};

pub struct GetOpenLotsResponse {
    pub open_lots: HashMap<Symbol, Vec<Arc<RwLock<Lot>>>>,
    pub missing_symbols: Vec<Symbol>,
}

pub enum InventoryEvent {
    OpenLot {
        order_id: OrderId,
        batch_order_id: BatchOrderId,
        lot_id: LotId,
        symbol: Symbol,
        side: Side,
        price: Amount,
        quantity: Amount,
        fee: Amount,
        original_batch_quantity: Amount,
        batch_quantity_remaining: Amount,
        timestamp: DateTime<Utc>,
    },
    CloseLot {
        original_order_id: OrderId,
        original_batch_order_id: BatchOrderId,
        original_lot_id: LotId,
        closing_order_id: OrderId,
        closing_batch_order_id: BatchOrderId,
        closing_lot_id: LotId,
        symbol: Symbol,
        side: Side,
        original_price: Amount,     // original price when lot was opened
        closing_price: Amount,      // price in this closing event
        closing_fee: Amount,        // fee paid for closing event
        quantity_closed: Amount,    // quantity closed in this event
        original_quantity: Amount,  // original quantity when lot was opened
        quantity_remaining: Amount, // quantity remaining in the lot
        closing_batch_original_quantity: Amount,
        closing_batch_quantity_remaining: Amount,
        original_timestamp: DateTime<Utc>,
        closing_timestamp: DateTime<Utc>,
    },
}

pub struct InventoryManager {
    pub observer: SingleObserver<InventoryEvent>,
    pub order_tracker: Arc<RwLock<OrderTracker>>,
    pub open_lots: HashMap<Symbol, VecDeque<Arc<RwLock<Lot>>>>,
    pub closed_lots: HashMap<Symbol, VecDeque<Arc<RwLock<Lot>>>>,
    pub tolerance: Amount,
}

impl InventoryManager {
    pub fn new(order_tracker: Arc<RwLock<OrderTracker>>, tolerance: Amount) -> Self {
        Self {
            observer: SingleObserver::new(),
            order_tracker,
            open_lots: HashMap::new(),
            closed_lots: HashMap::new(),
            tolerance,
        }
    }

    /// notify about new lots to subscriber (-> Solver)
    pub fn notify_allocated_lots(&self) {}

    fn create_lot(
        &mut self,
        order_id: OrderId,
        batch_order_id: BatchOrderId,
        lot_id: LotId,
        symbol: Symbol,
        side: Side,
        price_filled: Amount,
        quantity_filled: Amount,
        fee_paid: Amount,
        original_quantity: Amount,
        quantity_remaining: Amount,
        fill_timestamp: DateTime<Utc>,
    ) {
        // We must open lots on the Long side (Buy) only as we don't support short-selling.
        assert_eq!(side, Side::Buy);

        let lot = Arc::new(RwLock::new(Lot {
            original_order_id: order_id.clone(),
            original_batch_order_id: batch_order_id.clone(),
            lot_id: lot_id.clone(),
            symbol: symbol.clone(),
            side,
            original_price: price_filled,
            original_quantity: quantity_filled,
            original_fee: fee_paid,
            remaining_quantity: quantity_filled,
            created_timestamp: fill_timestamp,
            last_update_timestamp: fill_timestamp,
            lot_transactions: Vec::new(),
        }));

        match self.open_lots.entry(symbol.clone()) {
            Entry::Occupied(mut entry) => {
                entry.get_mut().push_back(lot);
            }
            Entry::Vacant(entry) => {
                entry.insert(vec![lot].into());
            }
        };

        self.observer.publish_single(InventoryEvent::OpenLot {
            order_id,
            batch_order_id,
            lot_id,
            symbol,
            side,
            price: price_filled,
            quantity: quantity_filled,
            fee: fee_paid,
            original_batch_quantity: original_quantity,
            batch_quantity_remaining: quantity_remaining,
            timestamp: fill_timestamp,
        });
    }

    /// Match fill against remaining quantity within currently open lots.
    /// Returns any unmatched quantity.
    fn match_lots(
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
    ) -> Result<Option<Amount>> {
        // We must close lots on the Long side only, and we do that by selling them (Sell).
        assert_eq!(side, Side::Sell);

        match self.open_lots.entry(symbol.clone()) {
            Entry::Occupied(mut entry) => {
                let lots = entry.get_mut();

                while let Some(lot) = lots.back().cloned() {
                    let lot_quantity_remaining = lot.read().remaining_quantity;

                    let remaining_quantity = lot_quantity_remaining
                        .checked_sub(quantity_filled)
                        .ok_or(eyre!("Math overflow"))?;

                    let (matched_lot_quantity, lot_quantity_remaining, finished) =
                        if remaining_quantity < self.tolerance {
                            // We closed the lot
                            lots.pop_back();

                            // ...move it to closed_lots
                            self.closed_lots
                                .entry(symbol.clone())
                                .and_modify(|closed_lots| closed_lots.push_back(lot.clone()))
                                .or_insert([lot.clone()].into());

                            (
                                // we matched whole lot
                                lot_quantity_remaining,
                                // there is no quantity remaining on the lot
                                Amount::ZERO,

                                // check if we should continue
                                if remaining_quantity < -self.tolerance {
                                    // there is some quantity left to continue matching
                                    quantity_filled = quantity_filled
                                        .checked_add(remaining_quantity)
                                        .ok_or(eyre!("Math overflow"))?;

                                    false
                                } else {
                                    // fill was fully matched
                                    quantity_filled = Amount::ZERO;
                                    true
                                },
                            )
                        } else {
                            // We matched whole quantity of fill
                            let matched_lot_quantity = quantity_filled;

                            // fill was fully matched
                            quantity_filled = Amount::ZERO;
                            
                            // We partly closed the lot
                            (matched_lot_quantity, remaining_quantity, true)
                        };

                    let mut lot_write = lot.write();

                    lot_write.lot_transactions.push(LotTransaction {
                        order_id: order_id.clone(),
                        batch_order_id: batch_order_id.clone(),
                        matched_lot_id: lot_id.clone(),
                        closing_fee: fee_paid,
                        closing_price: price_filled,
                        closing_timestamp: fill_timestamp,
                        quantity_closed: matched_lot_quantity,
                    });

                    lot_write.last_update_timestamp = fill_timestamp;
                    lot_write.remaining_quantity = remaining_quantity;

                    self.observer.publish_single(InventoryEvent::CloseLot {
                        original_order_id: lot_write.original_order_id.clone(),
                        original_batch_order_id: lot_write.original_batch_order_id.clone(),
                        original_lot_id: lot_write.lot_id.clone(),
                        closing_order_id: order_id.clone(),
                        closing_batch_order_id: batch_order_id.clone(),
                        closing_lot_id: lot_id.clone(),
                        symbol: symbol.clone(),
                        side: lot_write.side,
                        original_price: lot_write.original_price,
                        closing_price: price_filled,
                        closing_fee: fee_paid,
                        quantity_closed: quantity_filled,
                        original_quantity: lot_write.original_quantity,
                        quantity_remaining: remaining_quantity,
                        original_timestamp: lot_write.created_timestamp,
                        closing_batch_original_quantity: batch_original_quantity,
                        closing_batch_quantity_remaining: batch_quantity_remaining,
                        closing_timestamp: fill_timestamp,
                    });
                
                    if finished {
                        break;
                    }
                }

                if self.tolerance < quantity_filled {
                    // We didn't fully match this fill against open lots, which means we're going Short, and
                    // this should never happen!
                    Ok(Some(quantity_filled))
                } else {
                    // We fully matched the incoming fill against open lots, and no quantity is remaining
                    // Note: This should be expected reusult.
                    Ok(None)
                }
            }
            Entry::Vacant(_) => {
                // We didn't match any lots, because there is no lots open for this symbol, which means
                // we're going Short, and this should never happen!
                Ok(Some(quantity_filled))
            }
        }
    }

    /// receive fill reports from Order Tracker
    pub fn handle_fill_report(
        &mut self,
        notification: OrderTrackerNotification,
    ) -> Result<Option<Amount>> {
        // 1. match against lots (in case of Sell), P&L report
        // 2. allocate new lots, store Cost/Fees
        //self.notify_lots(&[Lot::default()]);
        match notification {
            OrderTrackerNotification::Fill {
                order_id,
                batch_order_id,
                lot_id,
                symbol,
                side,
                price_filled,
                quantity_filled,
                fee_paid,
                original_quantity,
                quantity_remaining,
                fill_timestamp,
            } => {
                // open or close lot (lot matching)
                match side {
                    Side::Buy => {
                        // open new lot
                        // send OpenLot event to subscriber (-> Solver)
                        self.create_lot(
                            order_id,
                            batch_order_id,
                            lot_id,
                            symbol,
                            side,
                            price_filled,
                            quantity_filled,
                            fee_paid,
                            original_quantity,
                            quantity_remaining,
                            fill_timestamp,
                        );
                        // We opened new lots, no unmatched quantity to report
                        Ok(None)
                    }
                    Side::Sell => {
                        // match against open lots, close lots
                        // send CloseLot event to subscriber (-> Solver)
                        self.match_lots(
                            order_id,
                            batch_order_id,
                            lot_id,
                            symbol,
                            side,
                            price_filled,
                            quantity_filled,
                            fee_paid,
                            original_quantity,
                            quantity_remaining,
                            fill_timestamp,
                        )
                    }
                }
            }

            OrderTrackerNotification::Cancel {
                order_id: _,
                batch_order_id: _,
                symbol: _,
                side: _,
                quantity_cancelled: _,
                original_quantity: _,
                quantity_remaining: _,
                cancel_timestamp: _,
            } => {
                // TBD: Cancel doesn't open or close any lots. It's just a
                // notification to subscriber that order was cancelled
                // Perhaps Solver will subscribe directly to OrderTracker for Cancells
                // if required. OrderTracker needs to receive cancels from OrderConnector
                // to track remaining quantity and whether order is live or not, but
                // aside from that cancells may not be of any interest.
                // We didn't work with lots, so no unmatched quantity to report.
                Ok(None)
            }
        }
    }

    /// receive new order requests from Solver
    pub fn new_order(&self, basket_order: Arc<BatchOrder>) -> Result<()> {
        // Start writing to Order Tracker
        let mut guard = self.order_tracker.write();
        // Send all orders out
        for asset_order in &basket_order.asset_orders {
            guard
                .new_order(Arc::new(SingleOrder {
                    order_id: asset_order.order_id.clone(),
                    batch_order_id: basket_order.batch_order_id.clone(),
                    symbol: asset_order.symbol.clone(),
                    side: asset_order.side,
                    price: asset_order.price,
                    quantity: asset_order.quantity,
                    created_timestamp: basket_order.created_timestamp,
                }))
                .or(Err(eyre!(
                    "Failed to create new order for {} in basket {}",
                    asset_order.symbol,
                    basket_order.batch_order_id
                )))?;
        }
        // All orders out
        Ok(())
    }

    /// provide method to get open lots
    pub fn get_open_lots(&self, symbols: &[Symbol]) -> GetOpenLotsResponse {
        // Optimistic: we should be able to find all symbols
        let mut open_lots = HashMap::with_capacity(symbols.len());

        // ...but first we copy all symbols into a Vec
        let mut missing_symbols = symbols.to_vec();

        // ...and then we should collect open lots, and find the missing ones
        let partition_point = partition(&mut missing_symbols, |symbol| {
            if let Some(lots) = self.open_lots.get(symbol) {
                open_lots.insert(symbol.clone(), lots.iter().cloned().collect());
                false
            } else {
                true
            }
        });

        missing_symbols.splice(partition_point.., []);

        GetOpenLotsResponse {
            open_lots,
            missing_symbols,
        }
    }
}

#[cfg(test)]
mod test {

    #[test]
    fn test_inventory_manager() {}
}
