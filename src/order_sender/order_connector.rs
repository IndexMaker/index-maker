use std::sync::Arc;

use crate::core::bits::{Amount, LotId, Order, OrderId, Side, Symbol};
use eyre::Result;

/// abstract, allow sending orders and cancels, receiving acks, naks, executions
pub enum OrderConnectorNotification {
    Fill {
        order_id: OrderId,
        lot_id: LotId,
        symbol: Symbol,
        side: Side,
        price: Amount,
        quantity: Amount,
    },
    Cancel {
        order_id: OrderId,
        symbol: Symbol,
        side: Side,
        quantity: Amount,
    },
}

pub trait OrderConnector {
    // Send order to exchange (-> Binance)
    fn send_order(&mut self, order: &Arc<Order>) -> Result<()>;
}

#[cfg(test)]
pub mod test_util {

    use std::sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    };

    use crate::core::{
        bits::{Amount, LotId, Order, OrderId, Side, Symbol},
        functional::SingleObserver,
    };
    use eyre::Result;

    use super::{OrderConnector, OrderConnectorNotification};

    pub struct MockOrderConnector {
        pub observer: SingleObserver<OrderConnectorNotification>,
        pub implementor: SingleObserver<Arc<Order>>,
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

        /// Receive fills from exchange, and publish an event to subscrber (-> Order Tracker)
        pub fn notify_fill(
            &self,
            order_id: OrderId,
            lot_id: LotId,
            symbol: Symbol,
            side: Side,
            price: Amount,
            quantity: Amount,
        ) {
            self.observer
                .publish_single(OrderConnectorNotification::Fill {
                    order_id,
                    lot_id,
                    symbol,
                    side,
                    price,
                    quantity,
                });
        }

        /// Recive cancel from exchange and publish an event to subscrber (-> Order Tracker)
        pub fn notify_cancel(&self, order_id: OrderId, symbol: Symbol, side: Side, quantity: Amount) {
            self.observer
                .publish_single(OrderConnectorNotification::Cancel {
                    order_id,
                    symbol,
                    side,
                    quantity,
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
        fn send_order(&mut self, order: &Arc<Order>) -> Result<()> {
            self.implementor.publish_single(order.clone());
            Ok(())
        }
    }
}

#[cfg(test)]
pub mod test {
    use std::sync::{atomic::Ordering, Arc};

    use parking_lot::RwLock;

    use crate::{
        assert_decimal_approx_eq,
        core::{
            bits::{ClientOrderId, LotId, Order, OrderId, Side},
            test_util::{
                flag_mock_atomic_bool, get_mock_asset_name_1, get_mock_atomic_bool_pair,
                get_mock_decimal, get_mock_defer_channel, run_mock_deferred, test_mock_atomic_bool,
            },
        },
    };

    use super::{test_util::MockOrderConnector, OrderConnector, OrderConnectorNotification};

    #[test]
    fn test_mock_order_connector() {
        let (flag_1, flag_2) = get_mock_atomic_bool_pair();
        let (tx_1, rx) = get_mock_defer_channel();
        let tx_2 = tx_1.clone();

        let order_connector_1 = Arc::new(RwLock::new(MockOrderConnector::new()));
        let order_connector_2 = Arc::downgrade(&order_connector_1);

        let order_price = get_mock_decimal("100.0");
        let order_quantity = get_mock_decimal("50.0");
        let fill_quantity = order_quantity
            .checked_mul(get_mock_decimal("0.75"))
            .unwrap();
        let cancel_quantity = order_quantity.checked_sub(fill_quantity).unwrap();
        let lot_id_1 = LotId("Lot01".into());
        let lot_id_2 = lot_id_1.clone();

        order_connector_1
            .write()
            .implementor
            .set_observer_fn(move |e: Arc<Order>| {
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
                    );
                    order_connector.read().notify_cancel(
                        e.order_id.clone(),
                        e.symbol.clone(),
                        Side::Buy,
                        cancel_quantity,
                    );
                }))
                .unwrap();
            });

        let order_id_1 = OrderId("Mock01".into());
        let order_id_2 = order_id_1.clone();

        order_connector_1
            .write()
            .observer
            .set_observer_fn(move |e: OrderConnectorNotification| {
                let flag = flag_2.clone();
                let order_id_2 = order_id_2.clone();
                let lot_id_2 = lot_id_2.clone();
                tx_2.send(Box::new(move || {
                    let tolerance = get_mock_decimal("0.01");
                    match e {
                        OrderConnectorNotification::Fill {
                            symbol,
                            order_id,
                            lot_id,
                            side,
                            price,
                            quantity,
                        } => {
                            assert_eq!(symbol, get_mock_asset_name_1());
                            assert_eq!(side, Side::Buy);
                            assert_eq!(order_id, order_id_2);
                            assert_eq!(lot_id, lot_id_2);
                            assert_decimal_approx_eq!(price, order_price, tolerance);
                            assert_decimal_approx_eq!(quantity, fill_quantity, tolerance);
                        }
                        OrderConnectorNotification::Cancel {
                            order_id,
                            symbol,
                            side,
                            quantity,
                        } => {
                            flag_mock_atomic_bool(&flag);
                            assert_eq!(symbol, get_mock_asset_name_1());
                            assert_eq!(side, Side::Buy);
                            assert_eq!(order_id, order_id_2);
                            assert_decimal_approx_eq!(quantity, cancel_quantity, tolerance);
                        }
                    };
                }))
                .unwrap();
            });

        order_connector_1.write().connect();

        assert!(order_connector_1
            .read()
            .is_connected
            .load(Ordering::Relaxed));

        order_connector_1.write().send_order(&Arc::new(Order {
            order_id: order_id_1.clone(),
            client_order_id: ClientOrderId("MockOrder".into()),
            symbol: get_mock_asset_name_1(),
            side: Side::Buy,
            price: order_price,
            quantity: order_quantity,
        })).unwrap();

        run_mock_deferred(rx);
        test_mock_atomic_bool(&flag_1);
    }
}
