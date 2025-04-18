use std::{collections::VecDeque, sync::Arc};

use chrono::{DateTime, Utc};
use eyre::{eyre, Result};
use parking_lot::RwLock;

use crate::core::bits::{Address, Amount, ClientOrderId, PaymentId, Side, Symbol};

pub enum PaymentDirection {
    /// Credit the giver: they gave us money, that increases our liability
    /// towards them. If they credit their account with us, we owe them index.
    Credit,
    /// Debit the receiver: they receive money, that decreases our liability
    /// towards them. If they debit their account, we get their index.
    Debit,
}

pub struct Payment {
    /// On-chain wallet address
    pub address: Address,

    /// An ID of this payment
    pub payment_id: PaymentId,

    /// Credit or Debit
    pub direction: PaymentDirection,

    /// Amount paid from(to) custody to(from) user wallet
    pub amount: Amount,
}

pub struct IndexOrderUpdate {
    /// On-chain wallet address
    ///
    /// An address of subsequent buyer / seller. Users may have exchange token
    /// on-chain, and ownership is split.
    pub address: Address,

    /// ID of the update assigned by the user (<- FIX)
    pub client_order_id: ClientOrderId,

    /// An ID of the corresponding payment
    ///
    /// Note: In case of Buy it is an ID allocated for the payment that will
    /// come from them to cover for the transaction. And in case of Sell, there
    /// will be ID allocated to identify the payment that we will make to them
    /// in relationship with this update.
    pub payment_id: PaymentId,

    /// Buy or Sell
    pub side: Side,

    /// Limit price
    pub price: Amount,

    /// Price max deviation %-age (as fraction) threshold
    pub price_threshold: Amount,

    /// Quantity of an index to buy or sell
    pub original_quantity: Amount,

    /// Quantity remaining after applying matching update
    pub remaining_quantity: Amount,

    /// Quantity engaged by Solver
    pub engaged_quantity: Option<Amount>,

    /// Quantity confirmed as filled
    pub filled_quantity: Amount,

    /// Fee for updating the order
    pub update_fee: Amount,

    /// Timestamp
    pub timestamp: DateTime<Utc>,
}

/// An order to buy index
pub struct IndexOrder {
    /// On-chain wallet address
    ///
    /// An address of the first user who had the index created. First buyer.
    pub original_address: Address,

    /// ID of the Index Order assigned by the user (<- FIX)
    pub original_client_order_id: ClientOrderId,

    /// An index symbol
    pub symbol: Symbol,

    /// Buy or Sell
    pub side: Side,

    /// Quantity remaining on the order
    pub remaining_quantity: Amount,

    /// Time of when this order was created
    pub created_timestamp: DateTime<Utc>,

    /// Time of the last update to this order
    pub last_update_timestamp: DateTime<Utc>,

    /// Side engaged by Solver
    pub engaged_side: Option<Side>,

    /// Quantity engaged by Solver
    pub engaged_quantity: Option<Amount>,

    /// Order updates
    pub order_updates: VecDeque<Arc<RwLock<IndexOrderUpdate>>>,

    /// Past order updates
    pub closed_updates: VecDeque<Arc<RwLock<IndexOrderUpdate>>>,
}

impl IndexOrder {
    /// Create brand new Index order
    pub fn new(
        address: Address,
        client_order_id: ClientOrderId,
        symbol: Symbol,
        side: Side,
        timestamp: DateTime<Utc>,
    ) -> Self {
        Self {
            original_address: address,
            original_client_order_id: client_order_id,
            symbol,
            side,
            remaining_quantity: Amount::ZERO,
            created_timestamp: timestamp.clone(),
            last_update_timestamp: timestamp.clone(),
            engaged_side: None,
            engaged_quantity: None,
            order_updates: VecDeque::new(),
            closed_updates: VecDeque::new(),
        }
    }

    /// Add an update for an existing Index order
    pub fn update_order(
        &mut self,
        address: Address,
        client_order_id: ClientOrderId,
        payment_id: PaymentId,
        side: Side,
        price: Amount,
        price_threshold: Amount,
        quantity: Amount,
        timestamp: DateTime<Utc>,
        tolerance: Amount,
    ) -> Result<(Amount, Amount)> {
        // Allocate new Index order update
        let index_order_update = Arc::new(RwLock::new(IndexOrderUpdate {
            address: address.clone(),
            client_order_id: client_order_id.clone(),
            payment_id: payment_id.clone(),
            side,
            price,
            price_threshold,
            original_quantity: quantity,
            remaining_quantity: quantity,
            engaged_quantity: None,
            filled_quantity: Amount::ZERO,
            update_fee: Amount::ZERO,
            timestamp: timestamp.clone(),
        }));
        if self.side.opposite_side() == side {
            // Match against updates when on the opposite side
            if let Some(unmatched_quantity) = self.match_updates(quantity, tolerance)? {
                if let Some(_) = self.engaged_quantity {
                    // We consumed all available updates, and the other updates were engaged by Solver
                    self.remaining_quantity = Amount::ZERO;
                    let quantity_removed = quantity
                        .checked_sub(unmatched_quantity)
                        .ok_or(eyre!("Math overflow"))?;
                    // (quantity removed, quantity added)
                    Ok((quantity_removed, Amount::ZERO))
                } else {
                    // We consumed all updates and created new on opposite side when flipped the side
                    index_order_update.write().remaining_quantity = unmatched_quantity;
                    self.order_updates.push_back(index_order_update.clone());
                    self.side = side;
                    let original_quantity = self.remaining_quantity;
                    self.remaining_quantity = unmatched_quantity;
                    // (quantity removed, quantity added)
                    Ok((original_quantity, unmatched_quantity))
                }
            } else {
                // We consumed some number of updates
                self.remaining_quantity = self
                    .remaining_quantity
                    .checked_sub(quantity)
                    .ok_or(eyre!("Math overflow"))?;
                // (quantity removed, quantity added)
                Ok((quantity, Amount::ZERO))
            }
        } else {
            // Add remaining quantity when on the same side
            self.remaining_quantity = self
                .remaining_quantity
                .checked_add(quantity)
                .ok_or(eyre!("Math overflow"))?;
            self.order_updates.push_back(index_order_update.clone());
            // (quantity removed, quantity added)
            Ok((Amount::ZERO, quantity))
        }
    }

    pub fn cancel_updates(
        &mut self,
        quantity: Amount,
        tolerance: Amount,
    ) -> Result<(bool, Amount)> {
        if let Some(unmatched_quantity) = self.match_updates(quantity, tolerance)? {
            let quantity_removed = quantity
                .checked_sub(unmatched_quantity)
                .ok_or(eyre!("Math overflow"))?;
            if let Some(_) = self.engaged_quantity {
                // We consumed all available updates, and the other updates were engaged by Solver
                self.remaining_quantity = Amount::ZERO;
                Ok((false, quantity_removed))
            } else {
                // complete cancel
                Ok((true, quantity_removed))
            }
        } else {
            self.remaining_quantity = self
                .remaining_quantity
                .checked_add(quantity)
                .ok_or(eyre!("Math overflow"))?;
            // partial cancel
            Ok((false, quantity))
        }
    }

    fn match_updates(&mut self, mut quantity: Amount, tolerance: Amount) -> Result<Option<Amount>> {
        while let Some(update) = self.order_updates.back().cloned() {
            // quantity remaining on the update
            let future_remaining_quantity = update
                .read()
                .remaining_quantity
                .checked_sub(quantity)
                .ok_or(eyre!("Math overflow"))?;

            if future_remaining_quantity < tolerance {
                // Check if Solver engaged with this update
                //
                // Note we don't need to check if quantity remaining on
                // this update would be >0. Solver when picks up an order update
                // would update both remaining_quantity and engaged_quantity.
                //
                if let Some(_) = update.read().engaged_quantity {
                    // We stop on this update
                    //
                    // Note we set remaining quantity to 0, so that
                    // in the next iteration Solver will not prepare
                    // any new orders for this update.
                    //
                    update.write().remaining_quantity = Amount::ZERO;
                    return Ok(Some(-future_remaining_quantity));
                } else {
                    // We fully closed that update
                    let update = self.order_updates.pop_back().unwrap();
                    self.closed_updates.push_back(update);

                    if future_remaining_quantity < -tolerance {
                        // there's more quantity on the incoming update left
                        quantity = -future_remaining_quantity;
                    } else {
                        // we consumed whole incoming update
                        return Ok(None);
                    }
                }
            } else {
                // We partly closed that update
                //
                // Note even if Solver engaged with this update, it would
                // change the remaining_quantity to reflect the amount it
                // engaged with. So that the amount of remaining quantity
                // would be left for next iteration, i.e. no orders were
                // prepared yet to cover for remaining_quantity. Orders that
                // were prepared cover engaged_quantity.
                let mut update = update.write();
                update.remaining_quantity = future_remaining_quantity;
                break;
            }
        }
        Ok(Some(quantity))
    }

    pub fn engage(&mut self, quantity: Amount) -> Result<()> {
        Ok(())
    }
}
