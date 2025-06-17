use std::collections::HashSet;
use std::sync::Arc;

use chrono::{DateTime, Utc};
use eyre::{eyre, OptionExt, Result};
use safe_math::safe;
use std::collections::{hash_map::Entry, HashMap};

use crossbeam::atomic::AtomicCell;
use parking_lot::RwLock;

use crate::core::functional::{IntoObservableSingle, PublishSingle};
use crate::core::{
    bits::{Amount, BatchOrderId, OrderId, Side, SingleOrder, Symbol},
    decimal_ext::DecimalExt,
};
use crate::order_sender::order_connector::SessionId;
use crate::solver::position::LotId;
use crate::{
    core::functional::SingleObserver,
    order_sender::order_connector::{OrderConnector, OrderConnectorNotification},
};

/// track orders that we sent to
pub enum OrderTrackerNotification {
    Fill {
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
        is_cancelled: bool,
        fill_timestamp: DateTime<Utc>,
    },
    Cancel {
        order_id: OrderId,
        batch_order_id: BatchOrderId,
        symbol: Symbol,
        side: Side,
        quantity_cancelled: Amount,
        original_quantity: Amount,
        quantity_remaining: Amount,
        is_cancelled: bool,
        cancel_timestamp: DateTime<Utc>,
    },
}

#[derive(Clone, Copy)]
pub enum OrderStatus {
    Live { quantity_remaining: Amount },
    Cancelled { quantity_remaining: Amount },
    SendFailed,
}

pub struct OrderEntry {
    pub session_id: SessionId,
    pub order: Arc<SingleOrder>,
    status: AtomicCell<OrderStatus>,
}

impl OrderEntry {
    pub fn new(session_id: SessionId, order: &Arc<SingleOrder>) -> Self {
        Self {
            session_id,
            order: order.clone(),
            status: AtomicCell::new(OrderStatus::Live {
                quantity_remaining: order.quantity,
            }),
        }
    }

    pub fn set_status(&self, status: OrderStatus) {
        self.status.store(status);
    }

    pub fn get_status(&self) -> OrderStatus {
        self.status.load()
    }
}

pub struct OrderTracker {
    observer: SingleObserver<OrderTrackerNotification>,
    pub order_connector: Arc<RwLock<dyn OrderConnector>>,
    pub session: Option<SessionId>,
    pub orders: HashMap<OrderId, Arc<OrderEntry>>,
    pub tolerance: Amount,
}

impl OrderTracker {
    pub fn new(order_connector: Arc<RwLock<dyn OrderConnector>>, tolerance: Amount) -> Self {
        Self {
            observer: SingleObserver::new(),
            order_connector,
            session: None, // TODO: Support multiple sessions
            orders: HashMap::new(),
            tolerance,
        }
    }

    fn update_order_status(
        &mut self,
        order_id: OrderId,
        quantity: Amount,
        is_cancel: bool,
    ) -> Result<(Arc<OrderEntry>, Amount, bool, bool)> {
        match self.orders.entry(order_id.clone()) {
            // We're receiving a Fill or Cancel for an order, so we should be able to find it our records
            Entry::Occupied(entry) => {
                let order_entry = entry.get();
                match order_entry.get_status() {
                    // It makes sense that only live orders can be filled or cancelled
                    OrderStatus::Live { quantity_remaining } => {
                        if let Some(quantity_remaining) = safe!(quantity_remaining - quantity) {
                            // Should the remaining quantity on the order be zero, we deem it cancelled
                            if quantity_remaining < self.tolerance {
                                order_entry
                                    .set_status(OrderStatus::Cancelled { quantity_remaining });
                                Ok((order_entry.clone(), quantity_remaining, true, true))
                            } else {
                                // ...otherwise there is quantity left, and we deem it alive
                                order_entry.set_status(OrderStatus::Live { quantity_remaining });
                                // We return quantity remaining to avoid unnecessary matching of status by the caller
                                Ok((order_entry.clone(), quantity_remaining, true, false))
                            }
                        } else {
                            Err(eyre!("Math overflow"))
                        }
                    }
                    _ => {
                        if is_cancel && quantity < self.tolerance {
                            Ok((order_entry.clone(), Amount::ZERO, false, true))
                        } else {
                            Err(eyre!(
                                "We shouldn't be getting fills for an order that was cancelled"
                            ))
                        }
                    }
                }
            }
            Entry::Vacant(_) => Err(eyre!("Untracked order")),
        }
    }

    /// Receive execution reports from OrderConnector
    pub fn handle_order_notification(
        &mut self,
        notification: OrderConnectorNotification,
    ) -> Result<()> {
        match notification {
            OrderConnectorNotification::SessionLogon {
                session_id,
                // TODO: maybe we include list of assets and markets, and
                // account status then new_order() could chose which session to
                // send order.
            } => {
                println!("(order-tracker) Session connected: {}", session_id);
                if let Some(prev_sid) = self.session.replace(session_id) {
                    eprintln!("(order-tracker) Dropping previous session: {}", prev_sid);
                }
                Ok(())
            }
            OrderConnectorNotification::SessionLogout { session_id, reason } => {
                println!(
                    "(order-tracker) Session diconnected: {}, Reason: {}",
                    session_id, reason
                );
                self.session = None;
                Ok(())
            }
            OrderConnectorNotification::Fill {
                order_id,
                lot_id,
                symbol,
                side,
                price,
                quantity,
                fee,
                timestamp,
            } => {
                // Update book keeping of all orders we sent to exchange (-> Binance Order Connector)
                match self.update_order_status(order_id.clone(), quantity, false) {
                    Ok((order_entry, quantity_remaining, _, is_cancelled)) => {
                        // Notify about fills sending notification to subscriber (-> Inventory Manager)
                        self.observer
                            .publish_single(OrderTrackerNotification::Fill {
                                order_id,
                                lot_id,
                                batch_order_id: order_entry.order.batch_order_id.clone(),
                                symbol,
                                side,
                                original_quantity: order_entry.order.quantity,
                                price_filled: price,
                                fee_paid: fee,
                                quantity_filled: quantity,
                                quantity_remaining,
                                is_cancelled,
                                fill_timestamp: timestamp,
                            });
                        Ok(())
                    }
                    Err(err) => Err(eyre!("Error for {} {}", order_id, err)),
                }
            }
            OrderConnectorNotification::Cancel {
                order_id,
                symbol,
                side,
                quantity,
                timestamp,
            } => {
                // Update book keeping of all orders we sent to exchange (-> Binance Order Connector)
                match self.update_order_status(order_id.clone(), quantity, true) {
                    Ok((order_entry, quantity_remaining, was_live, is_cancelled)) => {
                        // Notify about fills sending notification to subscriber (-> Inventory Manager)
                        if was_live {
                            self.observer
                                .publish_single(OrderTrackerNotification::Cancel {
                                    order_id,
                                    batch_order_id: order_entry.order.batch_order_id.clone(),
                                    symbol,
                                    side,
                                    original_quantity: order_entry.order.quantity,
                                    quantity_cancelled: quantity,
                                    quantity_remaining,
                                    is_cancelled,
                                    cancel_timestamp: timestamp,
                                });
                        } // otherwise we already notified in the fill
                        Ok(())
                    }
                    Err(err) => Err(eyre!("Error for {} {}", order_id, err)),
                }
            }
        }
    }

    /// Receive new order requests from InventoryManager
    pub fn new_order(&mut self, order: Arc<SingleOrder>) -> Result<()> {
        let session_id = self
            .session
            .clone()
            .ok_or_eyre("No connected session available")?;
        match self.orders.entry(order.order_id.clone()) {
            Entry::Occupied(_) => Err(eyre!("Order already sent with ID {}", order.order_id)),
            Entry::Vacant(entry) => {
                // We create an entry in our records to book keeping
                let order_entry = Arc::new(OrderEntry {
                    session_id: session_id.clone(),
                    order: order.clone(),
                    status: AtomicCell::new(OrderStatus::Live {
                        quantity_remaining: order.quantity,
                    }),
                });
                entry.insert(order_entry.clone());
                // ...and then we send the order after the entry added to our records
                match self.order_connector.write().send_order(session_id, &order) {
                    Err(err) => {
                        // ...so that we can track if order sending failed too
                        order_entry.status.store(OrderStatus::SendFailed);
                        Err(err)
                    }
                    // ...or succeeded
                    Ok(_) => Ok(()),
                    // Node: caller has assigned some OrderID to this order, so we don't return any value
                }
            }
        }
    }

    pub fn get_order(&self, order_id: &OrderId) -> Option<Arc<SingleOrder>> {
        self.orders
            .get(order_id)
            .and_then(|x| Some(x.order.clone()))
    }
}

impl IntoObservableSingle<OrderTrackerNotification> for OrderTracker {
    fn get_single_observer_mut(&mut self) -> &mut SingleObserver<OrderTrackerNotification> {
        &mut self.observer
    }
}

#[cfg(test)]
mod test {
    use std::sync::Arc;

    use chrono::Utc;
    use parking_lot::RwLock;
    use rust_decimal::dec;

    use crate::{
        assert_decimal_approx_eq,
        core::{
            bits::{Side, SingleOrder},
            functional::IntoObservableSingle,
            test_util::{
                flag_mock_atomic_bool, get_mock_asset_name_1, get_mock_atomic_bool_pair,
                get_mock_defer_channel, run_mock_deferred, test_mock_atomic_bool,
            },
        },
        order_sender::order_connector::{
            test_util::MockOrderConnector, OrderConnectorNotification, SessionId,
        },
        solver::position::LotId,
    };

    use super::{OrderTracker, OrderTrackerNotification};

    #[test]
    fn test_order_tracker() {
        let (defer_1, deferred_actions) = get_mock_defer_channel();
        let defer_2 = defer_1.clone();
        let defer_3 = defer_1.clone();

        let (flag_fill_1, flag_fill_2) = get_mock_atomic_bool_pair();
        let (flag_cancel_1, flag_cancel_2) = get_mock_atomic_bool_pair();

        let tolerance = dec!(0.0001);
        let order_connector = Arc::new(RwLock::new(MockOrderConnector::new()));

        let timestamp = Utc::now();
        let fee = dec!(0.10);

        let order_price = dec!(100.0);
        let order_quantity = dec!(50.0);
        let fill_quantity = order_quantity.checked_mul(dec!(0.75)).unwrap();
        let cancel_quantity = order_quantity.checked_sub(fill_quantity).unwrap();

        let order_1 = Arc::new(SingleOrder {
            order_id: "Mock01".into(),
            batch_order_id: "MockOrder".into(),
            symbol: get_mock_asset_name_1(),
            price: order_price,
            quantity: order_quantity,
            side: Side::Buy,
            created_timestamp: timestamp,
        });

        let order_2 = order_1.clone();
        let lot_id_1: LotId = "Lot01".into();
        let lot_id_2 = lot_id_1.clone();

        // Let's provide internal (mocked) implementation of the Order Connector
        // It will fill some portion of the order, and it will cancel the rest.
        let order_connector_weak = Arc::downgrade(&order_connector);
        order_connector.write().implementor.set_observer_fn(
            move |(sid, e): (SessionId, Arc<SingleOrder>)| {
                let lot_id_1 = lot_id_1.clone();
                let order_connector = order_connector_weak.upgrade().unwrap();
                assert_eq!(sid, SessionId("Session-01".to_owned()));
                defer_1
                    .send(Box::new(move || {
                        order_connector.read().notify_fill(
                            e.order_id.clone(),
                            lot_id_1.clone(),
                            e.symbol.clone(),
                            e.side,
                            e.price,
                            fill_quantity,
                            fee,
                            timestamp,
                        );
                        order_connector.read().notify_cancel(
                            e.order_id.clone(),
                            e.symbol.clone(),
                            e.side,
                            cancel_quantity,
                            timestamp,
                        );
                    }))
                    .unwrap();
            },
        );

        // Let's setup our Order Tracker (unit under test)
        let order_tracker = Arc::new(RwLock::new(OrderTracker::new(
            order_connector.clone(),
            tolerance,
        )));

        // We need to connect events from Order Connector to -> Order Tracker
        let order_tracker_weak = Arc::downgrade(&order_tracker);
        order_connector
            .write()
            .get_single_observer_mut()
            .set_observer_fn(move |e: OrderConnectorNotification| {
                let order_tracker = order_tracker_weak.upgrade().unwrap();
                defer_2
                    .send(Box::new(move || {
                        order_tracker.write().handle_order_notification(e).unwrap();
                    }))
                    .unwrap();
            });

        // We will expect some events in return
        order_tracker
            .write()
            .observer
            .set_observer_fn(move |e: OrderTrackerNotification| {
                let order = order_2.clone();
                let lot_id_2 = lot_id_2.clone();
                let flag_fill = flag_fill_2.clone();
                let flag_cancel = flag_cancel_2.clone();
                defer_3
                    .send(Box::new(move || match e {
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
                            is_cancelled,
                            fill_timestamp,
                        } => {
                            flag_mock_atomic_bool(&flag_fill);
                            assert_eq!(order_id, order.order_id);
                            assert_eq!(lot_id, lot_id_2);
                            assert_eq!(batch_order_id, order.batch_order_id);
                            assert_eq!(symbol, order.symbol);
                            assert_eq!(side, Side::Buy);
                            assert_eq!(price_filled, order_price);
                            assert_eq!(quantity_filled, fill_quantity);
                            assert_decimal_approx_eq!(fee_paid, fee, tolerance);
                            assert_eq!(original_quantity, order.quantity);
                            assert_eq!(fill_timestamp, timestamp);
                            assert_decimal_approx_eq!(
                                quantity_remaining,
                                order.quantity.checked_sub(fill_quantity).unwrap(),
                                tolerance
                            );
                        }
                        OrderTrackerNotification::Cancel {
                            order_id,
                            batch_order_id,
                            symbol,
                            side,
                            quantity_cancelled,
                            original_quantity,
                            quantity_remaining,
                            is_cancelled,
                            cancel_timestamp,
                        } => {
                            flag_mock_atomic_bool(&flag_cancel);
                            assert_eq!(order_id, order.order_id);
                            assert_eq!(batch_order_id, order.batch_order_id);
                            assert_eq!(symbol, order.symbol);
                            assert_eq!(side, Side::Buy);
                            assert_eq!(quantity_cancelled, cancel_quantity);
                            assert_eq!(original_quantity, order.quantity);
                            assert_eq!(cancel_timestamp, timestamp);
                            assert_decimal_approx_eq!(
                                quantity_remaining,
                                order
                                    .quantity
                                    .checked_sub(fill_quantity)
                                    .and_then(|x| x.checked_sub(cancel_quantity))
                                    .unwrap(),
                                tolerance
                            );
                        }
                    }))
                    .unwrap();
            });

        order_connector.write().notify_logon("Session-01".into());
        run_mock_deferred(&deferred_actions);

        order_tracker
            .write()
            .new_order(order_1.clone())
            .expect("Failed to send order");
        run_mock_deferred(&deferred_actions);

        order_connector
            .write()
            .notify_logout("Session-01".into(), "Session disconnected".to_owned());
        run_mock_deferred(&deferred_actions);

        test_mock_atomic_bool(&flag_fill_1);
        test_mock_atomic_bool(&flag_cancel_1);
    }
}
