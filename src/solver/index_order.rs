use std::{collections::VecDeque, sync::Arc};

use chrono::{DateTime, Utc};
use eyre::{OptionExt, Result};
use parking_lot::RwLock;
use safe_math::safe;

use crate::core::{
    bits::{Address, Amount, ClientOrderId, PaymentId, Side, Symbol},
    decimal_ext::DecimalExt,
};

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

    /// Quantity confirmed as filled
    pub filled_quantity: Amount,

    /// Fee for updating the order
    pub fees: Amount,

    /// Order updates (may contain partly cancelled, and partly engaged update)
    pub order_updates: VecDeque<Arc<RwLock<IndexOrderUpdate>>>,

    /// Fully engaged updates
    pub engaged_updates: VecDeque<Arc<RwLock<IndexOrderUpdate>>>,

    /// Fully closed order updates
    pub closed_updates: VecDeque<Arc<RwLock<IndexOrderUpdate>>>,
}

pub enum UpdateIndexOrderOutcome {
    Push {
        new_quantity: Amount,
    },
    Reduce {
        removed_quantity: Amount,
        remaining_quantity: Amount,
    },
    Flip {
        side: Side,
        new_quantity: Amount,
    },
}

pub enum CancelIndexOrderOutcome {
    Reduce {
        removed_quantity: Amount,
        remaining_quantity: Amount,
    },
    Cancel {
        removed_quantity: Amount,
    },
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
            filled_quantity: Amount::ZERO,
            fees: Amount::ZERO,
            order_updates: VecDeque::new(),
            engaged_updates: VecDeque::new(),
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
    ) -> Result<UpdateIndexOrderOutcome> {
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
            if let Some(unmatched_quantity) = self.match_cancel(quantity, tolerance)? {
                if self.engaged_side.is_some() {
                    // Solver is engaged in processing of this order
                    self.remaining_quantity = Amount::ZERO;
                    let quantity_removed =
                        safe!(quantity - unmatched_quantity).ok_or_eyre("Math Problem")?;
                    // (quantity removed, quantity added)
                    Ok(UpdateIndexOrderOutcome::Reduce {
                        removed_quantity: quantity_removed,
                        remaining_quantity: self.remaining_quantity,
                    })
                } else {
                    // We consumed all updates and created new on opposite side when flipped the side
                    index_order_update.write().remaining_quantity = unmatched_quantity;
                    self.order_updates.push_back(index_order_update);
                    // ...and we flipped unmatched quantity to the other side
                    self.remaining_quantity = unmatched_quantity;
                    self.side = side;
                    // (quantity removed, quantity added)
                    Ok(UpdateIndexOrderOutcome::Flip {
                        side,
                        new_quantity: unmatched_quantity,
                    })
                }
            } else {
                // We consumed some number of updates, so we cancelled that quantity
                self.remaining_quantity =
                    safe!(self.remaining_quantity - quantity).ok_or_eyre("Math Problem")?;
                // (quantity removed, quantity added)
                Ok(UpdateIndexOrderOutcome::Reduce {
                    removed_quantity: quantity,
                    remaining_quantity: self.remaining_quantity,
                })
            }
        } else {
            // We added some extra quantity on current side
            self.remaining_quantity =
                safe!(self.remaining_quantity + quantity).ok_or_eyre("Math Problem")?;
            self.order_updates.push_back(index_order_update);
            // (quantity removed, quantity added)
            Ok(UpdateIndexOrderOutcome::Push {
                new_quantity: quantity,
            })
        }
    }

    /// Cancel some updates to an existing Index order
    pub fn cancel_updates(
        &mut self,
        quantity: Amount,
        tolerance: Amount,
    ) -> Result<CancelIndexOrderOutcome> {
        // Match against updates
        if let Some(unmatched_quantity) = self.match_cancel(quantity, tolerance)? {
            // We consumed all available updates
            let quantity_removed =
                safe!(quantity - unmatched_quantity).ok_or_eyre("Math Problem")?;
            if self.engaged_side.is_some() {
                // Solver is engaged in processing of this order
                self.remaining_quantity = Amount::ZERO;
                // (is order cancelled, quantity cancelled)
                Ok(CancelIndexOrderOutcome::Reduce {
                    removed_quantity: quantity_removed,
                    remaining_quantity: self.remaining_quantity,
                })
            } else {
                Ok(CancelIndexOrderOutcome::Cancel {
                    removed_quantity: quantity_removed,
                })
            }
        } else {
            self.remaining_quantity =
                safe!(self.remaining_quantity + quantity).ok_or_eyre("Math Problem")?;
            Ok(CancelIndexOrderOutcome::Reduce {
                removed_quantity: quantity,
                remaining_quantity: self.remaining_quantity,
            })
        }
    }

    /// Engage
    pub fn solver_engage(&mut self, quantity: Amount, tolerance: Amount) -> Result<Option<Amount>> {
        if let Some(unmatched_quantity) = self.match_engage(quantity, tolerance)? {
            let engaged_quantity =
                safe!(quantity - unmatched_quantity).ok_or_eyre("Math Problem")?;
            safe!(self.engaged_quantity += engaged_quantity).ok_or_eyre("Math Problem")?;
            self.engaged_side = Some(self.side);
            Ok(Some(unmatched_quantity))
        } else {
            safe!(self.engaged_quantity += quantity).ok_or_eyre("Math Problem")?;
            self.engaged_side = Some(self.side);
            Ok(None)
        }
    }

    pub fn solver_cancel(&mut self, reason: &str) {
        self.closed_updates.extend(self.order_updates.drain(..));
        //todo!("figure this one out - solver didn't like this order")
        println!(
            "Error in Order: {} {}",
            self.original_client_order_id, reason
        );
    }

    /// Drain
    pub fn drain_closed_updates(&mut self, cb: &impl Fn(Arc<RwLock<IndexOrderUpdate>>)) {
        self.closed_updates.drain(..).for_each(cb);
    }

    /// Match updates against quantity and cancel
    /// Note that we are cancelling updates in LIFO order
    fn match_cancel(&mut self, mut quantity: Amount, tolerance: Amount) -> Result<Option<Amount>> {
        while let Some(update) = self.order_updates.back().cloned() {
            // begin transaction on update
            let mut update = update.write();

            // quantity remaining on the update
            let future_remaining_quantity =
                safe!(update.remaining_quantity - quantity).ok_or_eyre("Math Problem")?;

            if future_remaining_quantity < tolerance {
                // Check if Solver engaged with this update
                //
                // Note we don't need to check if quantity remaining on
                // this update would be >0. Solver when picks up an order update
                // would update both remaining_quantity and engaged_quantity.
                //
                if update.engaged_quantity.is_some() {
                    // We stop on this update
                    //
                    // Note we set remaining quantity to 0, so that
                    // in the next iteration Solver will not prepare
                    // any new orders for this update.
                    //
                    update.remaining_quantity = Amount::ZERO;
                    // This update is fully engaged at this moment
                    self.engaged_updates
                        .push_back(self.order_updates.pop_back().unwrap());
                    return Ok(Some(-future_remaining_quantity));
                } else {
                    // We fully closed that update
                    self.closed_updates
                        .push_back(self.order_updates.pop_back().unwrap());

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
                update.remaining_quantity = future_remaining_quantity;
                return Ok(None);
            }
        }
        Ok(Some(quantity))
    }

    /// Match updates against quantity and engage
    /// Note that we are engaging updates in FIFO order
    fn match_engage(&mut self, mut quantity: Amount, tolerance: Amount) -> Result<Option<Amount>> {
        while let Some(update) = self.order_updates.front().cloned() {
            // begin transaction on update
            let mut update = update.write();

            let future_remaining_quantity =
                safe!(update.remaining_quantity - quantity).ok_or_eyre("Math Problem")?;

            if future_remaining_quantity < tolerance {
                // We can engage with whole remaining quantity on this update
                let remaining_quantity = update.remaining_quantity;
                println!("Should update!");
                safe!(update.engaged_quantity += remaining_quantity).ok_or_eyre("Math Problem")?;

                // No quantity remaining on this update
                update.remaining_quantity = Amount::ZERO;

                // Move that update to fully engaged queue
                self.engaged_updates
                    .push_back(self.order_updates.pop_front().unwrap());

                if future_remaining_quantity < -tolerance {
                    // There is some quantity left for next iteration
                    quantity = -future_remaining_quantity;
                } else {
                    // We engaged all quantity
                    return Ok(None);
                }
            } else {
                // We can engage with whole quantity
                safe!(update.engaged_quantity += quantity).ok_or_eyre("Math Problem")?;
                update.remaining_quantity = future_remaining_quantity;
                return Ok(None);
            };
        }
        Ok(Some(quantity))
    }
}

#[cfg(test)]
mod test {
    use chrono::Utc;
    use rust_decimal::dec;

    use crate::{
        assert_decimal_approx_eq,
        core::{
            bits::{Amount, Side},
            test_util::{get_mock_address_1, get_mock_asset_name_1},
        },
        solver::index_order::UpdateIndexOrderOutcome,
    };

    use super::IndexOrder;

    /// Test sanity of unengaged IndexOrder
    ///
    /// In this test we will confirm that IndexOrder queue is managed correctly
    /// for case when Solver has not yet engaged with this IndexOrder.
    ///
    /// We create two Buy orders:
    ///    #1 Buy 10 @ $100
    ///    #2 Buy 20 @ $110
    ///
    /// These two orders will open:
    ///    * Buy 30
    ///
    /// Next we create Sell orders:
    ///    #3 Sell  5 @ $140
    ///    #4 Sell 20 @ $140
    ///
    /// The #3 Sell 5 @ $140 will match against #2 Buy 20 @ $110, leaving #2 Buy 15 @ $110.
    /// The #4 Sell 20 @ $140 will then match:
    ///     * fully against remaining #2 Buy 15 @ $110
    ///     * and partly against #1 Buy 10 @ $100, leaving #1 Buy 5 @ $100.
    ///
    /// Then we create Sell order:
    ///     #5 Sell 20 @ 150
    ///
    /// And that order will:
    ///     * fully match against remaining #1 Buy 5  @ $100
    ///     * and create order #5 Sell 15 @ $150, flipping order side to Sell
    ///
    /// The final result will be that order, which was initially created as Buy,
    /// now has become Sell order.
    ///
    /// Note this is possible only if Solver has not yet engaged in this order.
    ///
    #[test]
    fn test_index_order_1() {
        let tolerance = dec!(0.00001);

        let address = get_mock_address_1();
        let order_id = "Order01".into();
        let symbol = get_mock_asset_name_1();
        let timestamp = Utc::now();

        let mut order = IndexOrder::new(address, order_id, symbol, Side::Buy, timestamp);

        // And first update on Buy side
        let order_id1 = "Order02".into();
        let pay_id1 = "Pay01".into();
        let price1 = dec!(100.0);
        let price_threshold1 = dec!(0.05);
        let quantity1 = dec!(10.0);
        let timestamp1 = Utc::now();

        {
            let update_index_order_outcome = order
                .update_order(
                    address,
                    order_id1,
                    pay_id1,
                    Side::Buy,
                    price1,
                    price_threshold1,
                    quantity1,
                    timestamp1,
                    tolerance,
                )
                .unwrap();

            assert!(matches!(
                update_index_order_outcome,
                UpdateIndexOrderOutcome::Push { new_quantity: _ }
            ));
            match update_index_order_outcome {
                UpdateIndexOrderOutcome::Push { new_quantity } => {
                    assert_decimal_approx_eq!(new_quantity, quantity1, tolerance);
                }
                _ => assert!(false),
            }
            assert!(matches!(order.side, Side::Buy));
            assert_decimal_approx_eq!(order.remaining_quantity, quantity1, tolerance);
            assert_eq!(order.order_updates.len(), 1);
        }

        // Add another update on Buy side
        let order_id2 = "Order03".into();
        let pay_id2 = "Pay02".into();
        let price2 = dec!(110.0);
        let price_threshold2 = dec!(0.075);
        let quantity2 = dec!(20.0);
        let timestamp2 = Utc::now();

        {
            let update_index_order_outcome = order
                .update_order(
                    address,
                    order_id2,
                    pay_id2,
                    Side::Buy,
                    price2,
                    price_threshold2,
                    quantity2,
                    timestamp2,
                    tolerance,
                )
                .unwrap();

            assert!(matches!(
                update_index_order_outcome,
                UpdateIndexOrderOutcome::Push { new_quantity: _ }
            ));
            match update_index_order_outcome {
                UpdateIndexOrderOutcome::Push { new_quantity } => {
                    assert_decimal_approx_eq!(new_quantity, quantity2, tolerance);
                }
                _ => assert!(false),
            }
            assert!(matches!(order.side, Side::Buy));
            assert_decimal_approx_eq!(order.remaining_quantity, quantity1 + quantity2, tolerance);
            assert_eq!(order.order_updates.len(), 2);
        }

        // Add small update on Sell side
        let order_id3 = "Order04".into();
        let pay_id3 = "Pay03".into();
        let price3 = dec!(140.0);
        let price_threshold3 = dec!(0.05);
        let quantity3 = dec!(5.0);
        let timestamp3 = Utc::now();

        {
            let update_index_order_outcome = order
                .update_order(
                    address,
                    order_id3,
                    pay_id3,
                    Side::Sell,
                    price3,
                    price_threshold3,
                    quantity3,
                    timestamp3,
                    tolerance,
                )
                .unwrap();

            assert!(matches!(
                update_index_order_outcome,
                UpdateIndexOrderOutcome::Reduce {
                    removed_quantity: _,
                    remaining_quantity: _
                }
            ));
            match update_index_order_outcome {
                UpdateIndexOrderOutcome::Reduce {
                    removed_quantity,
                    remaining_quantity,
                } => {
                    assert_decimal_approx_eq!(removed_quantity, quantity3, tolerance);
                    assert_decimal_approx_eq!(
                        remaining_quantity,
                        order.remaining_quantity,
                        tolerance
                    );
                }
                _ => assert!(false),
            }
            assert!(matches!(order.side, Side::Buy));
            assert_decimal_approx_eq!(
                order.remaining_quantity,
                quantity1 + quantity2 - quantity3,
                tolerance
            );
            assert_eq!(order.order_updates.len(), 2);
        }

        // Add bigger update on Sell side
        let order_id4 = "Order05".into();
        let pay_id4 = "Pay04".into();
        let price4 = dec!(140.0);
        let price_threshold4 = dec!(0.05);
        let quantity4 = dec!(20.0);
        let timestamp4 = Utc::now();

        {
            let update_index_order_outcome = order
                .update_order(
                    address,
                    order_id4,
                    pay_id4,
                    Side::Sell,
                    price4,
                    price_threshold4,
                    quantity4,
                    timestamp4,
                    tolerance,
                )
                .unwrap();

            assert!(matches!(
                update_index_order_outcome,
                UpdateIndexOrderOutcome::Reduce {
                    removed_quantity: _,
                    remaining_quantity: _
                }
            ));
            match update_index_order_outcome {
                UpdateIndexOrderOutcome::Reduce {
                    removed_quantity,
                    remaining_quantity,
                } => {
                    assert_decimal_approx_eq!(removed_quantity, quantity4, tolerance);
                    assert_decimal_approx_eq!(
                        remaining_quantity,
                        order.remaining_quantity,
                        tolerance
                    );
                }
                _ => assert!(false),
            }
            assert!(matches!(order.side, Side::Buy));
            assert_decimal_approx_eq!(
                order.remaining_quantity,
                quantity1 + quantity2 - quantity3 - quantity4,
                tolerance
            );
            assert_eq!(order.order_updates.len(), 1);
        }

        // Another update on Sell side should flip
        let order_id5 = "Order06".into();
        let pay_id5 = "Pay05".into();
        let price5 = dec!(150.0);
        let price_threshold5 = dec!(0.05);
        let quantity5 = dec!(20.0);
        let timestamp5 = Utc::now();
        {
            let update_index_order_outcome = order
                .update_order(
                    address,
                    order_id5,
                    pay_id5,
                    Side::Sell,
                    price5,
                    price_threshold5,
                    quantity5,
                    timestamp5,
                    tolerance,
                )
                .unwrap();

            let quantity_added = quantity5 - (quantity1 + quantity2 - quantity3 - quantity4);

            assert!(matches!(
                update_index_order_outcome,
                UpdateIndexOrderOutcome::Flip {
                    side: _,
                    new_quantity: _
                }
            ));
            match update_index_order_outcome {
                UpdateIndexOrderOutcome::Flip { side, new_quantity } => {
                    assert!(matches!(side, Side::Sell));
                    assert_decimal_approx_eq!(new_quantity, quantity_added, tolerance);
                }
                _ => assert!(false),
            }
            assert!(matches!(order.side, Side::Sell));
            assert_decimal_approx_eq!(
                order.remaining_quantity,
                quantity5 - (quantity1 + quantity2 - quantity3 - quantity4),
                tolerance
            );
            assert_eq!(order.order_updates.len(), 1);
        }
    }

    /// Test sanity of engaged IndexOrder
    ///
    /// In this test we will confirm that IndexOrder queue is managed correctly
    /// for case when Solver has engaged with this IndexOrder.
    ///
    /// As in previous test we first create two Buy orders:
    ///    #1 Buy 10 @ $100
    ///    #2 Buy 20 @ $110
    ///
    /// These two orders will open:
    ///    * Buy 30
    ///
    /// After that we engage Solver in:
    ///     1.  5 quantity
    ///     2. 20 quantity
    ///
    /// First 1. 5 quantity will engage with #1 Buy 10 @ $100 blocking 5 and leaving 5 remaining.
    /// Second 2. 20 quantity will engage with:
    ///     * remaining 5 on #1 Buy 10 @ $100
    ///     * and 15 on #2 Buy 20 @ $110, leaving 5 remaining
    ///
    /// Next that we create Sell order:
    ///     #3 Sell 20 @ $140
    ///
    /// This #3 Sell order will:
    ///     * reduce #2 Buy 20 @ $110 from remaining 5 to 0, while keeping its 15 engaged
    ///     * and stop any further matching
    ///
    /// As the result both orders #1 and #2 will be fully engaged, and order will stay on Buy side.
    ///
    #[test]
    fn test_index_order_2() {
        let tolerance = dec!(0.00001);

        let address = get_mock_address_1();
        let order_id = "Order01".into();
        let symbol = get_mock_asset_name_1();
        let timestamp = Utc::now();

        let mut order = IndexOrder::new(address, order_id, symbol, Side::Buy, timestamp);

        // And first update on Buy side
        let order_id1 = "Order02".into();
        let pay_id1 = "Pay01".into();
        let price1 = dec!(100.0);
        let price_threshold1 = dec!(0.05);
        let quantity1 = dec!(10.0);
        let timestamp1 = Utc::now();

        {
            let update_index_order_outcome = order
                .update_order(
                    address,
                    order_id1,
                    pay_id1,
                    Side::Buy,
                    price1,
                    price_threshold1,
                    quantity1,
                    timestamp1,
                    tolerance,
                )
                .unwrap();

            assert!(matches!(
                update_index_order_outcome,
                UpdateIndexOrderOutcome::Push { new_quantity: _ }
            ));
            match update_index_order_outcome {
                UpdateIndexOrderOutcome::Push { new_quantity } => {
                    assert_decimal_approx_eq!(new_quantity, quantity1, tolerance);
                }
                _ => assert!(false),
            }
            assert!(matches!(order.side, Side::Buy));
            assert_decimal_approx_eq!(order.remaining_quantity, quantity1, tolerance);
            assert_eq!(order.order_updates.len(), 1);
        }
        // Add another update on Buy side
        let order_id2 = "Order03".into();
        let pay_id2 = "Pay02".into();
        let price2 = dec!(110.0);
        let price_threshold2 = dec!(0.075);
        let quantity2 = dec!(20.0);
        let timestamp2 = Utc::now();

        {
            let update_index_order_outcome = order
                .update_order(
                    address,
                    order_id2,
                    pay_id2,
                    Side::Buy,
                    price2,
                    price_threshold2,
                    quantity2,
                    timestamp2,
                    tolerance,
                )
                .unwrap();

            assert!(matches!(
                update_index_order_outcome,
                UpdateIndexOrderOutcome::Push { new_quantity: _ }
            ));
            match update_index_order_outcome {
                UpdateIndexOrderOutcome::Push { new_quantity } => {
                    assert_decimal_approx_eq!(new_quantity, quantity2, tolerance);
                }
                _ => assert!(false),
            }
            assert!(matches!(order.side, Side::Buy));
            assert_decimal_approx_eq!(order.remaining_quantity, quantity1 + quantity2, tolerance);
            assert_eq!(order.order_updates.len(), 2);
        }

        // Engage in small quantity
        let engage_quantity1 = dec!(5);
        {
            let unengaged_quantity1 = order.solver_engage(engage_quantity1, tolerance).unwrap();
            assert!(matches!(unengaged_quantity1, None));

            assert!(matches!(order.engaged_side, Some(Side::Buy)));
            assert_decimal_approx_eq!(order.engaged_quantity.unwrap(), engage_quantity1, tolerance);

            assert_eq!(order.order_updates.len(), 2);
            assert_eq!(order.engaged_updates.len(), 0);

            let mut iter = order.order_updates.iter();

            let update = iter.next().unwrap().read();
            assert!(update.engaged_quantity.is_some());
            assert_decimal_approx_eq!(
                update.engaged_quantity.unwrap(),
                engage_quantity1,
                tolerance
            );
            assert_decimal_approx_eq!(
                update.remaining_quantity,
                quantity1 - engage_quantity1,
                tolerance
            );

            let update = iter.next().unwrap().read();
            assert!(update.engaged_quantity.is_none());
        }

        // Engage in larger quantity
        let engage_quantity2 = dec!(20.0);
        {
            let unengaged_quantity2 = order.solver_engage(engage_quantity2, tolerance).unwrap();
            assert!(matches!(unengaged_quantity2, None));

            assert!(matches!(order.engaged_side, Some(Side::Buy)));
            assert_decimal_approx_eq!(
                order.engaged_quantity.unwrap(),
                engage_quantity1 + engage_quantity2,
                tolerance
            );

            assert_eq!(order.order_updates.len(), 1);
            assert_eq!(order.engaged_updates.len(), 1);

            let mut iter = order.order_updates.iter();

            let update = iter.next().unwrap().read();
            assert!(update.engaged_quantity.is_some());
            assert_decimal_approx_eq!(
                update.engaged_quantity.unwrap(),
                engage_quantity2 - (quantity1 - engage_quantity1),
                tolerance
            );
            assert_decimal_approx_eq!(
                update.remaining_quantity,
                quantity2 - (engage_quantity2 - (quantity1 - engage_quantity1)),
                tolerance
            );

            let mut iter = order.engaged_updates.iter();
            let update = iter.next().unwrap().read();
            assert_decimal_approx_eq!(update.engaged_quantity.unwrap(), quantity1, tolerance);
        }

        // At this stage first order is fully engaged, and second order is partly engaged.
        // Order is engaged on Buy side. We should not be able to flip sides or cancel any engaged quantity.
        {
            // Add bigger update on Sell side
            let order_id4 = "Order05".into();
            let pay_id4 = "Pay04".into();
            let price4 = dec!(140.0);
            let price_threshold4 = dec!(0.05);
            let quantity4 = dec!(20.0);
            let timestamp4 = Utc::now();

            let update_index_order_outcome = order
                .update_order(
                    address,
                    order_id4,
                    pay_id4,
                    Side::Sell,
                    price4,
                    price_threshold4,
                    quantity4,
                    timestamp4,
                    tolerance,
                )
                .unwrap();

            let quantity_removed = quantity2 - (engage_quantity2 - (quantity1 - engage_quantity1));

            assert!(matches!(
                update_index_order_outcome,
                UpdateIndexOrderOutcome::Reduce {
                    removed_quantity: _,
                    remaining_quantity: _
                }
            ));
            match update_index_order_outcome {
                UpdateIndexOrderOutcome::Reduce {
                    removed_quantity,
                    remaining_quantity,
                } => {
                    assert_decimal_approx_eq!(removed_quantity, quantity_removed, tolerance);
                    assert_decimal_approx_eq!(
                        remaining_quantity,
                        order.remaining_quantity,
                        tolerance
                    );
                }
                _ => assert!(false),
            }
            assert!(matches!(order.side, Side::Buy));
            assert_decimal_approx_eq!(order.remaining_quantity, Amount::ZERO, tolerance);

            // order was moved to fully engaged orders
            assert_eq!(order.order_updates.len(), 0);
            assert_eq!(order.engaged_updates.len(), 2);

            let mut iter = order.engaged_updates.iter();

            let update = iter.next().unwrap().read();
            assert_decimal_approx_eq!(update.engaged_quantity.unwrap(), quantity1, tolerance);

            let update = iter.next().unwrap().read();
            assert!(update.engaged_quantity.is_some());
            assert_decimal_approx_eq!(
                update.engaged_quantity.unwrap(),
                engage_quantity2 - (quantity1 - engage_quantity1),
                tolerance
            );
            assert_decimal_approx_eq!(update.remaining_quantity, Amount::ZERO, tolerance);
        }
    }
}
