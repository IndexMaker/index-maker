use std::sync::Arc;

use crate::{
    core::bits::{Amount, OrderId, Side, SingleOrder, Symbol},
    order_sender::position::LotId,
    string_id,
};
use chrono::{DateTime, Utc};
use eyre::Result;

string_id!(SessionId);

/// abstract, allow sending orders and cancels, receiving acks, naks, executions
pub enum OrderConnectorNotification {
    SessionLogon {
        session_id: SessionId,
        timestamp: DateTime<Utc>,
    },
    SessionLogout {
        session_id: SessionId,
        reason: String,
        timestamp: DateTime<Utc>,
    },
    Rejected {
        order_id: OrderId,
        symbol: Symbol,
        side: Side,
        price: Amount,
        quantity: Amount,
        reason: String,
        timestamp: DateTime<Utc>,
    },
    Fill {
        order_id: OrderId,
        lot_id: LotId,
        symbol: Symbol,
        side: Side,
        price: Amount,
        quantity: Amount,
        fee: Amount,
        timestamp: DateTime<Utc>,
    },
    Cancel {
        order_id: OrderId,
        symbol: Symbol,
        side: Side,
        quantity: Amount,
        timestamp: DateTime<Utc>,
    },
}

pub trait OrderConnector: Send + Sync {
    // Send order to exchange (-> Binance)
    fn send_order(&mut self, session_id: SessionId, order: &Arc<SingleOrder>) -> Result<()>;
}

pub mod test_util {

    use std::sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    };

    use crate::{
        core::{
            bits::{Amount, OrderId, Side, SingleOrder, Symbol},
            functional::{IntoObservableSingle, PublishSingle, SingleObserver},
        },
        order_sender::{order_connector::SessionId, position::LotId},
    };
    use chrono::{DateTime, Utc};
    use eyre::Result;

    use super::{OrderConnector, OrderConnectorNotification};

    pub struct MockOrderConnector {
        observer: SingleObserver<OrderConnectorNotification>,
        pub implementor: SingleObserver<(SessionId, Arc<SingleOrder>)>,
        pub is_connected: AtomicBool,
    }

    impl MockOrderConnector {
        pub fn new() -> Self {
            Self {
                observer: SingleObserver::new(),
                implementor: SingleObserver::new(),
                is_connected: AtomicBool::new(false),
            }
        }

        pub fn notify_logon(&self, session_id: SessionId, timestamp: DateTime<Utc>) {
            self.observer
                .publish_single(OrderConnectorNotification::SessionLogon {
                    session_id,
                    timestamp,
                });
        }

        pub fn notify_logout(
            &self,
            session_id: SessionId,
            reason: String,
            timestamp: DateTime<Utc>,
        ) {
            self.observer
                .publish_single(OrderConnectorNotification::SessionLogout {
                    session_id,
                    reason,
                    timestamp,
                });
        }

        /// Receive fills from exchange, and publish an event to subscrber (-> Order Tracker)
        pub fn notify_fill(
            &self,
            order_id: OrderId,
            lot_id: LotId,
            symbol: Symbol,
            side: Side,
            price: Amount,
            quantity: Amount,
            fee: Amount,
            timestamp: DateTime<Utc>,
        ) {
            self.observer
                .publish_single(OrderConnectorNotification::Fill {
                    order_id,
                    lot_id,
                    symbol,
                    side,
                    price,
                    quantity,
                    fee,
                    timestamp,
                });
        }

        /// Recive cancel from exchange and publish an event to subscrber (-> Order Tracker)
        pub fn notify_cancel(
            &self,
            order_id: OrderId,
            symbol: Symbol,
            side: Side,
            quantity: Amount,
            timestamp: DateTime<Utc>,
        ) {
            self.observer
                .publish_single(OrderConnectorNotification::Cancel {
                    order_id,
                    symbol,
                    side,
                    quantity,
                    timestamp,
                });
        }

        /// Connect to exchange
        pub fn connect(&mut self) {
            // connect to exchange (-> Binance)
            self.is_connected.store(true, Ordering::Relaxed);
        }
    }

    impl OrderConnector for MockOrderConnector {
        /// Send orders
        fn send_order(&mut self, session_id: SessionId, order: &Arc<SingleOrder>) -> Result<()> {
            self.implementor.publish_single((session_id, order.clone()));
            Ok(())
        }
    }

    impl IntoObservableSingle<OrderConnectorNotification> for MockOrderConnector {
        fn get_single_observer_mut(&mut self) -> &mut SingleObserver<OrderConnectorNotification> {
            &mut self.observer
        }
    }
}

#[cfg(test)]
pub mod test {
    use std::sync::{atomic::Ordering, Arc};

    use chrono::Utc;
    use parking_lot::RwLock;
    use rust_decimal::dec;

    use crate::{
        assert_decimal_approx_eq,
        core::{
            bits::{OrderId, Side, SingleOrder},
            functional::IntoObservableSingle,
            test_util::{
                flag_mock_atomic_bool, get_mock_asset_name_1, get_mock_atomic_bool_pair,
                get_mock_defer_channel, run_mock_deferred, test_mock_atomic_bool,
            },
        },
        order_sender::{order_connector::SessionId, position::LotId},
    };

    use super::{test_util::MockOrderConnector, OrderConnector, OrderConnectorNotification};

    #[test]
    fn test_mock_order_connector() {
        let (flag_1, flag_2) = get_mock_atomic_bool_pair();
        let (tx_1, rx) = get_mock_defer_channel();
        let tx_2 = tx_1.clone();

        let order_connector_1 = Arc::new(RwLock::new(MockOrderConnector::new()));
        let order_connector_2 = Arc::downgrade(&order_connector_1);

        let order_price = dec!(100.0);
        let order_quantity = dec!(50.0);
        let fill_quantity = order_quantity.checked_mul(dec!(0.75)).unwrap();
        let cancel_quantity = order_quantity.checked_sub(fill_quantity).unwrap();
        let lot_id_1: LotId = "Lot01".into();
        let lot_id_2 = lot_id_1.clone();

        let timestamp = Utc::now();
        let fee = dec!(0.10);

        order_connector_1.write().implementor.set_observer_fn(
            move |(sid, e): (SessionId, Arc<SingleOrder>)| {
                let lot_id = lot_id_1.clone();
                let order_connector = order_connector_2.upgrade().unwrap();
                tx_1.send(Box::new(move || {
                    order_connector.read().notify_fill(
                        e.order_id.clone(),
                        lot_id,
                        e.symbol.clone(),
                        Side::Buy,
                        e.price,
                        fill_quantity,
                        fee,
                        timestamp,
                    );
                    order_connector.read().notify_cancel(
                        e.order_id.clone(),
                        e.symbol.clone(),
                        Side::Buy,
                        cancel_quantity,
                        timestamp,
                    );
                }))
                .unwrap();
            },
        );

        let order_id_1: OrderId = "Mock01".into();
        let order_id_2 = order_id_1.clone();

        order_connector_1
            .write()
            .get_single_observer_mut()
            .set_observer_fn(move |e: OrderConnectorNotification| {
                let flag = flag_2.clone();
                let order_id_2 = order_id_2.clone();
                let lot_id_2 = lot_id_2.clone();
                let fee_2 = fee.clone();
                let timestamp_2 = timestamp.clone();
                tx_2.send(Box::new(move || {
                    let tolerance = dec!(0.01);
                    match e {
                        OrderConnectorNotification::SessionLogon {
                            session_id,
                            timestamp,
                        } => {
                            assert_eq!(session_id, SessionId("Session-01".to_owned()));
                        }
                        OrderConnectorNotification::SessionLogout {
                            session_id,
                            reason,
                            timestamp,
                        } => {
                            assert_eq!(session_id, SessionId("Session-01".to_owned()));
                        }
                        OrderConnectorNotification::Rejected {
                            order_id,
                            symbol,
                            side,
                            price,
                            quantity,
                            reason,
                            timestamp,
                        } => {
                            todo!("Test rejected reason");
                        }
                        OrderConnectorNotification::Fill {
                            symbol,
                            order_id,
                            lot_id,
                            side,
                            price,
                            quantity,
                            fee,
                            timestamp,
                        } => {
                            assert_eq!(symbol, get_mock_asset_name_1());
                            assert_eq!(side, Side::Buy);
                            assert_eq!(order_id, order_id_2);
                            assert_eq!(lot_id, lot_id_2);
                            assert_eq!(timestamp, timestamp_2);
                            assert_decimal_approx_eq!(price, order_price, tolerance);
                            assert_decimal_approx_eq!(quantity, fill_quantity, tolerance);
                            assert_decimal_approx_eq!(fee, fee_2, tolerance);
                        }
                        OrderConnectorNotification::Cancel {
                            order_id,
                            symbol,
                            side,
                            quantity,
                            timestamp,
                        } => {
                            flag_mock_atomic_bool(&flag);
                            assert_eq!(symbol, get_mock_asset_name_1());
                            assert_eq!(side, Side::Buy);
                            assert_eq!(order_id, order_id_2);
                            assert_eq!(timestamp, timestamp_2);
                            assert_decimal_approx_eq!(quantity, cancel_quantity, tolerance);
                        }
                    };
                }))
                .unwrap();
            });

        order_connector_1.write().connect();
        order_connector_1
            .write()
            .notify_logon("Session-01".into(), timestamp);

        assert!(order_connector_1
            .read()
            .is_connected
            .load(Ordering::Relaxed));

        order_connector_1
            .write()
            .send_order(
                "Session-01".into(),
                &Arc::new(SingleOrder {
                    order_id: order_id_1.clone(),
                    batch_order_id: "MockOrder".into(),
                    symbol: get_mock_asset_name_1(),
                    side: Side::Buy,
                    price: order_price,
                    quantity: order_quantity,
                    created_timestamp: timestamp,
                }),
            )
            .unwrap();

        order_connector_1.write().notify_logout(
            "Session-01".into(),
            "Session disconnected".to_owned(),
            timestamp,
        );

        run_mock_deferred(&rx);
        test_mock_atomic_bool(&flag_1);
    }
}
