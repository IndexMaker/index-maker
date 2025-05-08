use std::{collections::VecDeque, sync::Arc};

use chrono::{DateTime, Utc};
use eyre::{OptionExt, Result};
use parking_lot::RwLock;
use safe_math::safe;

use crate::core::{
    bits::{Address, Amount, ClientOrderId, PaymentId, Side, Symbol},
    decimal_ext::DecimalExt,
};

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

    /// Collateral for which to buy or sell index
    pub original_collateral_amount: Amount,

    /// Collateral remaining after applying matching update
    pub remaining_collateral: Amount,

    /// Collateral engaged by Solver
    pub engaged_collateral: Option<Amount>,

    /// Amount of collateral spent
    pub collateral_spent: Amount,

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

    /// Collateral remaining to spend on the order
    pub remaining_collateral: Amount,

    /// Time of when this order was created
    pub created_timestamp: DateTime<Utc>,

    /// Time of the last update to this order
    pub last_update_timestamp: DateTime<Utc>,

    /// Side engaged by Solver
    pub engaged_side: Option<Side>,

    /// Collateral amount engaged by Solver
    pub engaged_collateral: Option<Amount>,

    /// Amount of collateral spent
    pub collateral_spent: Amount,

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
        new_collateral_amount: Amount,
    },
    Reduce {
        removed_collateral: Amount,
        remaining_collateral: Amount,
    },
    Flip {
        side: Side,
        new_collateral_amount: Amount,
    },
}

pub enum CancelIndexOrderOutcome {
    Reduce {
        removed_collateral: Amount,
        remaining_collateral: Amount,
    },
    Cancel {
        removed_collateral: Amount,
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
            remaining_collateral: Amount::ZERO,
            created_timestamp: timestamp.clone(),
            last_update_timestamp: timestamp.clone(),
            engaged_side: None,
            engaged_collateral: None,
            collateral_spent: Amount::ZERO,
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
        collateral_amount: Amount,
        timestamp: DateTime<Utc>,
        tolerance: Amount,
    ) -> Result<UpdateIndexOrderOutcome> {
        // Allocate new Index order update
        let index_order_update = Arc::new(RwLock::new(IndexOrderUpdate {
            address: address.clone(),
            client_order_id: client_order_id.clone(),
            payment_id: payment_id.clone(),
            side,
            original_collateral_amount: collateral_amount,
            remaining_collateral: collateral_amount,
            engaged_collateral: None,
            collateral_spent: Amount::ZERO,
            filled_quantity: Amount::ZERO,
            update_fee: Amount::ZERO,
            timestamp: timestamp.clone(),
        }));
        if self.side.opposite_side() == side {
            // Match against updates when on the opposite side
            if let Some(unmatched_collateral) = self.match_cancel(collateral_amount, tolerance)? {
                if self.engaged_side.is_some() {
                    // Solver is engaged in processing of this order
                    self.remaining_collateral = Amount::ZERO;
                    let collateral_removed = safe!(collateral_amount - unmatched_collateral)
                        .ok_or_eyre("Math Problem")?;
                    Ok(UpdateIndexOrderOutcome::Reduce {
                        removed_collateral: collateral_removed,
                        remaining_collateral: self.remaining_collateral,
                    })
                } else {
                    // We consumed all updates and created new on opposite side when flipped the side
                    index_order_update.write().remaining_collateral = unmatched_collateral;
                    self.order_updates.push_back(index_order_update);
                    // ...and we flipped unmatched quantity to the other side
                    self.remaining_collateral = unmatched_collateral;
                    self.side = side;
                    Ok(UpdateIndexOrderOutcome::Flip {
                        side,
                        new_collateral_amount: unmatched_collateral,
                    })
                }
            } else {
                // We consumed some number of updates, so we cancelled that quantity
                self.remaining_collateral = safe!(self.remaining_collateral - collateral_amount)
                    .ok_or_eyre("Math Problem")?;
                Ok(UpdateIndexOrderOutcome::Reduce {
                    removed_collateral: collateral_amount,
                    remaining_collateral: self.remaining_collateral,
                })
            }
        } else {
            // We added some extra quantity on current side
            self.remaining_collateral =
                safe!(self.remaining_collateral + collateral_amount).ok_or_eyre("Math Problem")?;
            self.order_updates.push_back(index_order_update);
            Ok(UpdateIndexOrderOutcome::Push {
                new_collateral_amount: collateral_amount,
            })
        }
    }

    /// Cancel some updates to an existing Index order
    pub fn cancel_updates(
        &mut self,
        collateral_amount: Amount,
        tolerance: Amount,
    ) -> Result<CancelIndexOrderOutcome> {
        // Match against updates
        if let Some(unmatched_collateral) = self.match_cancel(collateral_amount, tolerance)? {
            // We consumed all available updates
            let collateral_removed =
                safe!(collateral_amount - unmatched_collateral).ok_or_eyre("Math Problem")?;
            if self.engaged_side.is_some() {
                // Solver is engaged in processing of this order
                self.remaining_collateral = Amount::ZERO;
                Ok(CancelIndexOrderOutcome::Reduce {
                    removed_collateral: collateral_removed,
                    remaining_collateral: self.remaining_collateral,
                })
            } else {
                Ok(CancelIndexOrderOutcome::Cancel {
                    removed_collateral: collateral_removed,
                })
            }
        } else {
            self.remaining_collateral =
                safe!(self.remaining_collateral + collateral_amount).ok_or_eyre("Math Problem")?;
            Ok(CancelIndexOrderOutcome::Reduce {
                removed_collateral: collateral_amount,
                remaining_collateral: self.remaining_collateral,
            })
        }
    }

    /// Engage
    pub fn solver_engage(
        &mut self,
        collateral_amount: Amount,
        tolerance: Amount,
    ) -> Result<Option<Amount>> {
        if let Some(unmatched_collateral) = self.match_engage(collateral_amount, tolerance)? {
            let engaged_collateral =
                safe!(collateral_amount - unmatched_collateral).ok_or_eyre("Math Problem")?;
            safe!(self.engaged_collateral += engaged_collateral).ok_or_eyre("Math Problem")?;
            self.remaining_collateral =
                safe!(self.remaining_collateral - engaged_collateral).ok_or_eyre("Math Problem")?;
            self.engaged_side = Some(self.side);
            Ok(Some(unmatched_collateral))
        } else {
            safe!(self.engaged_collateral += collateral_amount).ok_or_eyre("Math Problem")?;
            self.remaining_collateral =
                safe!(self.remaining_collateral - collateral_amount).ok_or_eyre("Math Problem")?;
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

    /// Match updates against collateral amount and cancel
    /// Note that we are cancelling updates in LIFO order
    fn match_cancel(
        &mut self,
        mut collateral_amount: Amount,
        tolerance: Amount,
    ) -> Result<Option<Amount>> {
        while let Some(update) = self.order_updates.back().cloned() {
            // begin transaction on update
            let mut update = update.write();

            // collateral remaining on the update
            let future_remaining_collateral =
                safe!(update.remaining_collateral - collateral_amount)
                    .ok_or_eyre("Math Problem")?;

            if future_remaining_collateral < tolerance {
                // Check if Solver engaged with this update
                //
                // Note we don't need to check if collateral remaining on
                // this update would be >0. Solver when picks up an order update
                // would update both remaining_collateral and engaged_collateral.
                //
                if update.engaged_collateral.is_some() {
                    // We stop on this update
                    //
                    // Note we set remaining collateral to 0, so that
                    // in the next iteration Solver will not prepare
                    // any new orders for this update.
                    //
                    update.remaining_collateral = Amount::ZERO;
                    // This update is fully engaged at this moment
                    self.engaged_updates
                        .push_back(self.order_updates.pop_back().unwrap());
                    return Ok(Some(-future_remaining_collateral));
                } else {
                    // We fully closed that update
                    self.closed_updates
                        .push_back(self.order_updates.pop_back().unwrap());

                    if future_remaining_collateral < -tolerance {
                        // there's more collateral on the incoming update left
                        collateral_amount = -future_remaining_collateral;
                    } else {
                        // we consumed whole incoming update
                        return Ok(None);
                    }
                }
            } else {
                // We partly closed that update
                //
                // Note even if Solver engaged with this update, it would
                // change the collateral_quantity to reflect the amount it
                // engaged with. So that the amount of remaining collateral
                // would be left for next iteration, i.e. no orders were
                // prepared yet to cover for remaining_collateral. Orders that
                // were prepared cover engaged_collateral.
                update.remaining_collateral = future_remaining_collateral;
                return Ok(None);
            }
        }
        Ok(Some(collateral_amount))
    }

    /// Match updates against collateral and engage
    /// Note that we are engaging updates in FIFO order
    fn match_engage(
        &mut self,
        mut collateral_amount: Amount,
        tolerance: Amount,
    ) -> Result<Option<Amount>> {
        while let Some(update) = self.order_updates.front().cloned() {
            // begin transaction on update
            let mut update = update.write();

            let future_remaining_collateral =
                safe!(update.remaining_collateral - collateral_amount)
                    .ok_or_eyre("Math Problem")?;

            if future_remaining_collateral < tolerance {
                // We can engage with whole remaining collateral on this update
                let remaining_collateral = update.remaining_collateral;
                println!("Should update!");
                safe!(update.engaged_collateral += remaining_collateral)
                    .ok_or_eyre("Math Problem")?;

                // No collateral remaining on this update
                update.remaining_collateral = Amount::ZERO;

                // Move that update to fully engaged queue
                self.engaged_updates
                    .push_back(self.order_updates.pop_front().unwrap());

                if future_remaining_collateral < -tolerance {
                    // There is some collateral left for next iteration
                    collateral_amount = -future_remaining_collateral;
                } else {
                    // We engaged all quantity
                    return Ok(None);
                }
            } else {
                // We can engage with whole quantity
                safe!(update.engaged_collateral += collateral_amount).ok_or_eyre("Math Problem")?;
                update.remaining_collateral = future_remaining_collateral;
                return Ok(None);
            };
        }
        Ok(Some(collateral_amount))
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
    ///    #1 Buy for $10
    ///    #2 Buy for $20
    ///
    /// These two orders will open:
    ///    * Buy for $30
    ///
    /// Next we create Sell orders:
    ///    #3 Sell for  $5
    ///    #4 Sell for $20
    ///
    /// The #3 Sell for $5 will match against #2 Buy for $20, leaving #2 Buy for $15.
    /// The #4 Sell for $20 will then match:
    ///     * fully against remaining #2 Buy for $15
    ///     * and partly against #1 Buy for $10, leaving #1 Buy for $5.
    ///
    /// Then we create Sell order:
    ///     #5 Sell for $20
    ///
    /// And that order will:
    ///     * fully match against remaining #1 Buy for $5
    ///     * and create order #5 Sell for $15, flipping order side to Sell
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
        let collateral1 = dec!(10.0);
        let timestamp1 = Utc::now();

        {
            let update_index_order_outcome = order
                .update_order(
                    address,
                    order_id1,
                    pay_id1,
                    Side::Buy,
                    collateral1,
                    timestamp1,
                    tolerance,
                )
                .unwrap();

            assert!(matches!(
                update_index_order_outcome,
                UpdateIndexOrderOutcome::Push {
                    new_collateral_amount: _
                }
            ));
            match update_index_order_outcome {
                UpdateIndexOrderOutcome::Push {
                    new_collateral_amount: new_collateral,
                } => {
                    assert_decimal_approx_eq!(new_collateral, collateral1, tolerance);
                }
                _ => assert!(false),
            }
            assert!(matches!(order.side, Side::Buy));
            assert_decimal_approx_eq!(order.remaining_collateral, collateral1, tolerance);
            assert_eq!(order.order_updates.len(), 1);
        }

        // Add another update on Buy side
        let order_id2 = "Order03".into();
        let pay_id2 = "Pay02".into();
        let collateral2 = dec!(20.0);
        let timestamp2 = Utc::now();

        {
            let update_index_order_outcome = order
                .update_order(
                    address,
                    order_id2,
                    pay_id2,
                    Side::Buy,
                    collateral2,
                    timestamp2,
                    tolerance,
                )
                .unwrap();

            assert!(matches!(
                update_index_order_outcome,
                UpdateIndexOrderOutcome::Push {
                    new_collateral_amount: _
                }
            ));
            match update_index_order_outcome {
                UpdateIndexOrderOutcome::Push {
                    new_collateral_amount: new_collateral,
                } => {
                    assert_decimal_approx_eq!(new_collateral, collateral2, tolerance);
                }
                _ => assert!(false),
            }
            assert!(matches!(order.side, Side::Buy));
            assert_decimal_approx_eq!(
                order.remaining_collateral,
                collateral1 + collateral2,
                tolerance
            );
            assert_eq!(order.order_updates.len(), 2);
        }

        // Add small update on Sell side
        let order_id3 = "Order04".into();
        let pay_id3 = "Pay03".into();
        let collateral3 = dec!(5.0);
        let timestamp3 = Utc::now();

        {
            let update_index_order_outcome = order
                .update_order(
                    address,
                    order_id3,
                    pay_id3,
                    Side::Sell,
                    collateral3,
                    timestamp3,
                    tolerance,
                )
                .unwrap();

            assert!(matches!(
                update_index_order_outcome,
                UpdateIndexOrderOutcome::Reduce {
                    removed_collateral: _,
                    remaining_collateral: _
                }
            ));
            match update_index_order_outcome {
                UpdateIndexOrderOutcome::Reduce {
                    removed_collateral,
                    remaining_collateral,
                } => {
                    assert_decimal_approx_eq!(removed_collateral, collateral3, tolerance);
                    assert_decimal_approx_eq!(
                        remaining_collateral,
                        order.remaining_collateral,
                        tolerance
                    );
                }
                _ => assert!(false),
            }
            assert!(matches!(order.side, Side::Buy));
            assert_decimal_approx_eq!(
                order.remaining_collateral,
                collateral1 + collateral2 - collateral3,
                tolerance
            );
            assert_eq!(order.order_updates.len(), 2);
        }

        // Add bigger update on Sell side
        let order_id4 = "Order05".into();
        let pay_id4 = "Pay04".into();
        let collateral4 = dec!(20.0);
        let timestamp4 = Utc::now();

        {
            let update_index_order_outcome = order
                .update_order(
                    address,
                    order_id4,
                    pay_id4,
                    Side::Sell,
                    collateral4,
                    timestamp4,
                    tolerance,
                )
                .unwrap();

            assert!(matches!(
                update_index_order_outcome,
                UpdateIndexOrderOutcome::Reduce {
                    removed_collateral: _,
                    remaining_collateral: _
                }
            ));
            match update_index_order_outcome {
                UpdateIndexOrderOutcome::Reduce {
                    removed_collateral,
                    remaining_collateral,
                } => {
                    assert_decimal_approx_eq!(removed_collateral, collateral4, tolerance);
                    assert_decimal_approx_eq!(
                        remaining_collateral,
                        order.remaining_collateral,
                        tolerance
                    );
                }
                _ => assert!(false),
            }
            assert!(matches!(order.side, Side::Buy));
            assert_decimal_approx_eq!(
                order.remaining_collateral,
                collateral1 + collateral2 - collateral3 - collateral4,
                tolerance
            );
            assert_eq!(order.order_updates.len(), 1);
        }

        // Another update on Sell side should flip
        let order_id5 = "Order06".into();
        let pay_id5 = "Pay05".into();
        let collateral5 = dec!(20.0);
        let timestamp5 = Utc::now();
        {
            let update_index_order_outcome = order
                .update_order(
                    address,
                    order_id5,
                    pay_id5,
                    Side::Sell,
                    collateral5,
                    timestamp5,
                    tolerance,
                )
                .unwrap();

            let collateral_added =
                collateral5 - (collateral1 + collateral2 - collateral3 - collateral4);

            assert!(matches!(
                update_index_order_outcome,
                UpdateIndexOrderOutcome::Flip {
                    side: _,
                    new_collateral_amount: _
                }
            ));
            match update_index_order_outcome {
                UpdateIndexOrderOutcome::Flip {
                    side,
                    new_collateral_amount: new_collateral,
                } => {
                    assert!(matches!(side, Side::Sell));
                    assert_decimal_approx_eq!(new_collateral, collateral_added, tolerance);
                }
                _ => assert!(false),
            }
            assert!(matches!(order.side, Side::Sell));
            assert_decimal_approx_eq!(
                order.remaining_collateral,
                collateral5 - (collateral1 + collateral2 - collateral3 - collateral4),
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
        let collateral1 = dec!(10.0);
        let timestamp1 = Utc::now();

        {
            let update_index_order_outcome = order
                .update_order(
                    address,
                    order_id1,
                    pay_id1,
                    Side::Buy,
                    collateral1,
                    timestamp1,
                    tolerance,
                )
                .unwrap();

            assert!(matches!(
                update_index_order_outcome,
                UpdateIndexOrderOutcome::Push {
                    new_collateral_amount: _
                }
            ));
            match update_index_order_outcome {
                UpdateIndexOrderOutcome::Push {
                    new_collateral_amount: new_collateral,
                } => {
                    assert_decimal_approx_eq!(new_collateral, collateral1, tolerance);
                }
                _ => assert!(false),
            }
            assert!(matches!(order.side, Side::Buy));
            assert_decimal_approx_eq!(order.remaining_collateral, collateral1, tolerance);
            assert_eq!(order.order_updates.len(), 1);
        }
        // Add another update on Buy side
        let order_id2 = "Order03".into();
        let pay_id2 = "Pay02".into();
        let collateral2 = dec!(20.0);
        let timestamp2 = Utc::now();

        {
            let update_index_order_outcome = order
                .update_order(
                    address,
                    order_id2,
                    pay_id2,
                    Side::Buy,
                    collateral2,
                    timestamp2,
                    tolerance,
                )
                .unwrap();

            assert!(matches!(
                update_index_order_outcome,
                UpdateIndexOrderOutcome::Push {
                    new_collateral_amount: _
                }
            ));
            match update_index_order_outcome {
                UpdateIndexOrderOutcome::Push {
                    new_collateral_amount: new_collateral,
                } => {
                    assert_decimal_approx_eq!(new_collateral, collateral2, tolerance);
                }
                _ => assert!(false),
            }
            assert!(matches!(order.side, Side::Buy));
            assert_decimal_approx_eq!(
                order.remaining_collateral,
                collateral1 + collateral2,
                tolerance
            );
            assert_eq!(order.order_updates.len(), 2);
        }

        // Engage in small collateral
        let engage_collateral1 = dec!(5);
        {
            let unengaged_collateral1 = order.solver_engage(engage_collateral1, tolerance).unwrap();
            assert!(matches!(unengaged_collateral1, None));

            assert!(matches!(order.engaged_side, Some(Side::Buy)));
            assert_decimal_approx_eq!(
                order.engaged_collateral.unwrap(),
                engage_collateral1,
                tolerance
            );

            assert_eq!(order.order_updates.len(), 2);
            assert_eq!(order.engaged_updates.len(), 0);

            let mut iter = order.order_updates.iter();

            let update = iter.next().unwrap().read();
            assert!(update.engaged_collateral.is_some());
            assert_decimal_approx_eq!(
                update.engaged_collateral.unwrap(),
                engage_collateral1,
                tolerance
            );
            assert_decimal_approx_eq!(
                update.remaining_collateral,
                collateral1 - engage_collateral1,
                tolerance
            );
            assert_decimal_approx_eq!(
                order.remaining_collateral,
                collateral1 + collateral2 - engage_collateral1,
                tolerance
            );

            let update = iter.next().unwrap().read();
            assert!(update.engaged_collateral.is_none());
        }

        // Engage in larger collateral
        let engage_collateral2 = dec!(20.0);
        {
            let unengaged_collateral2 = order.solver_engage(engage_collateral2, tolerance).unwrap();
            assert!(matches!(unengaged_collateral2, None));

            assert!(matches!(order.engaged_side, Some(Side::Buy)));
            assert_decimal_approx_eq!(
                order.engaged_collateral.unwrap(),
                engage_collateral1 + engage_collateral2,
                tolerance
            );

            assert_eq!(order.order_updates.len(), 1);
            assert_eq!(order.engaged_updates.len(), 1);

            let mut iter = order.order_updates.iter();

            let update = iter.next().unwrap().read();
            assert!(update.engaged_collateral.is_some());
            assert_decimal_approx_eq!(
                update.engaged_collateral.unwrap(),
                engage_collateral2 - (collateral1 - engage_collateral1),
                tolerance
            );
            assert_decimal_approx_eq!(
                update.remaining_collateral,
                collateral2 - (engage_collateral2 - (collateral1 - engage_collateral1)),
                tolerance
            );

            let mut iter = order.engaged_updates.iter();
            let update = iter.next().unwrap().read();
            assert_decimal_approx_eq!(update.engaged_collateral.unwrap(), collateral1, tolerance);
        }

        // At this stage first order is fully engaged, and second order is partly engaged.
        // Order is engaged on Buy side. We should not be able to flip sides or cancel any engaged collateral.
        {
            // Add bigger update on Sell side
            let order_id4 = "Order05".into();
            let pay_id4 = "Pay04".into();
            let collateral4 = dec!(20.0);
            let timestamp4 = Utc::now();

            let update_index_order_outcome = order
                .update_order(
                    address,
                    order_id4,
                    pay_id4,
                    Side::Sell,
                    collateral4,
                    timestamp4,
                    tolerance,
                )
                .unwrap();

            let collateral_removed =
                collateral2 - (engage_collateral2 - (collateral1 - engage_collateral1));

            assert!(matches!(
                update_index_order_outcome,
                UpdateIndexOrderOutcome::Reduce {
                    removed_collateral: _,
                    remaining_collateral: _
                }
            ));
            match update_index_order_outcome {
                UpdateIndexOrderOutcome::Reduce {
                    removed_collateral,
                    remaining_collateral,
                } => {
                    assert_decimal_approx_eq!(removed_collateral, collateral_removed, tolerance);
                    assert_decimal_approx_eq!(
                        remaining_collateral,
                        order.remaining_collateral,
                        tolerance
                    );
                }
                _ => assert!(false),
            }
            assert!(matches!(order.side, Side::Buy));
            assert_decimal_approx_eq!(order.remaining_collateral, Amount::ZERO, tolerance);

            // order was moved to fully engaged orders
            assert_eq!(order.order_updates.len(), 0);
            assert_eq!(order.engaged_updates.len(), 2);

            let mut iter = order.engaged_updates.iter();

            let update = iter.next().unwrap().read();
            assert_decimal_approx_eq!(update.engaged_collateral.unwrap(), collateral1, tolerance);

            let update = iter.next().unwrap().read();
            assert!(update.engaged_collateral.is_some());
            assert_decimal_approx_eq!(
                update.engaged_collateral.unwrap(),
                engage_collateral2 - (collateral1 - engage_collateral1),
                tolerance
            );
            assert_decimal_approx_eq!(update.remaining_collateral, Amount::ZERO, tolerance);
        }
    }
}
