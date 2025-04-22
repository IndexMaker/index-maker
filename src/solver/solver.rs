use std::sync::Arc;

use itertools::Itertools;
use parking_lot::RwLock;

use crate::{
    blockchain::chain_connector::{ChainConnector, ChainNotification},
    core::bits::{Amount, ClientOrderId, PriceType, Side},
    index::basket_manager::{BasketManager, BasketNotification},
    market_data::{
        order_book::order_book_manager::{OrderBookEvent, OrderBookManager},
        price_tracker::{PriceEvent, PriceTracker},
    },
};

use super::{
    index_order_manager::{IndexOrderEvent, IndexOrderManager},
    index_quote_manager::{QuoteRequestEvent, QuoteRequestManager},
    inventory_manager::{InventoryEvent, InventoryManager},
};

/// magic solver, needs to take index orders, and based on prices (from price
/// tracker) and available liquiduty (depth from order books), and active orders
/// (from order tracker) calculate best internal-portfolio rebalancing orders,
/// which will (partly) fill (some of the) ordered indexes.  Any position that
/// wasn't matched against ordered indexes shouldn't be kept for too long.
pub struct Solver {
    pub chain_connector: Arc<RwLock<dyn ChainConnector + Send + Sync>>,
    pub index_order_manager: Arc<RwLock<IndexOrderManager>>,
    pub quote_request_manager: Arc<RwLock<dyn QuoteRequestManager + Send + Sync>>,
    pub basket_manager: Arc<RwLock<BasketManager>>,
    pub price_tracker: Arc<RwLock<PriceTracker>>,
    pub order_book_manager: Arc<RwLock<dyn OrderBookManager + Send + Sync>>,
    pub inventory_manager: Arc<RwLock<InventoryManager>>,
}
impl Solver {
    pub fn new(
        chain_connector: Arc<RwLock<dyn ChainConnector + Send + Sync>>,
        index_order_manager: Arc<RwLock<IndexOrderManager>>,
        quote_request_manager: Arc<RwLock<dyn QuoteRequestManager + Send + Sync>>,
        basket_manager: Arc<RwLock<BasketManager>>,
        price_tracker: Arc<RwLock<PriceTracker>>,
        order_book_manager: Arc<RwLock<dyn OrderBookManager + Send + Sync>>,
        inventory_manager: Arc<RwLock<InventoryManager>>,
    ) -> Self {
        Self {
            chain_connector,
            index_order_manager,
            quote_request_manager,
            basket_manager,
            price_tracker,
            order_book_manager,
            inventory_manager,
        }
    }

    /// Core thinking function
    pub fn solve(&self) {
        // receive Index Orders
        let _index_orders = self.index_order_manager.read().get_pending_order_requests();

        // Compute symbols and threshold
        // ...

        let symbols = [];
        let threshold = Amount::default();

        // receive list of open lots from Inventory Manager
        let _positions = self.inventory_manager.read().get_positions(&symbols);

        // Compute: Allocate open lots to Index Orders
        // ...
        // TBD: Should Solver or Inventory Manager be allocating lots to index orders?

        // Send back to Index Order Manager fills if any
        self.index_order_manager
            .write()
            .fill_order_request(ClientOrderId::default(), Amount::default());

        // Compute: Remaining quantity
        // ...

        // receive current prices from Price Tracker
        let prices = self
            .price_tracker
            .read()
            .get_prices(PriceType::BestAsk, &symbols);

        // receive available liquidity from Order Book Manager
        let _liquidity =
            self.order_book_manager
                .read()
                .get_liquidity(Side::Sell, &prices.prices, threshold);

        // Compute: Orders to send to update inventory
        // ...

        // Send order requests to Inventory Manager
        // ...throttle these: send one or few smaller ones
        // TBD: Should throttling be done here in Solver or in Inventory Manager
        //self.inventory_manager.write().new_order();
    }

    /// Quoting function (fast)
    pub fn quote(&self, _quote_request: ()) {
        // Compute symbols and threshold
        // ...

        let symbols = [];
        let threshold = Amount::default();

        // receive current prices from Price Tracker
        let prices = self
            .price_tracker
            .read()
            .get_prices(PriceType::VolumeWeighted, &symbols);

        // receive available liquidity from Order Book Manager
        let _liquidity =
            self.order_book_manager
                .read()
                .get_liquidity(Side::Sell, &prices.prices, threshold);

        // Compute: Quote with cost
        // ...

        // send back quote
        self.quote_request_manager.write().respond_quote(());
    }

    pub fn handle_chain_event(&self, notification: ChainNotification) {
        println!("Solver: Handle Chain Event");
        match notification {
            ChainNotification::CuratorWeightsSet(symbol, basket_definition) => {
                let symbols = basket_definition
                    .weights
                    .iter()
                    .map(|w| w.asset.name.clone())
                    .collect_vec();

                let get_prices_response = self
                    .price_tracker
                    .read()
                    .get_prices(PriceType::VolumeWeighted, symbols.as_slice());

                if !get_prices_response.missing_symbols.is_empty() {
                    println!(
                        "Solver: No prices available for some symbols: {:?}",
                        get_prices_response.missing_symbols
                    );
                }

                let target_price = "1000".try_into().unwrap(); // TODO

                if let Err(err) = self.basket_manager.write().set_basket_from_definition(
                    symbol,
                    basket_definition,
                    &get_prices_response.prices,
                    target_price,
                ) {
                    println!("Solver: Error while setting curator weights: {err}");
                }
            }
            ChainNotification::PaymentIn {
                address: _,
                payment_id: _,
                amount_paid_in: _,
            } => (),
        }
    }

    /// receive Index Order
    pub fn handle_index_order(&self, _notification: IndexOrderEvent) {
        println!("Solver: Handle Index Order");
        //self.solve();
    }

    // receive QR
    pub fn handle_quote_request(&self, _notification: QuoteRequestEvent) {
        println!("Solver: Handle Quote Request");
        //self.quote(());
    }

    /// Receive fill notifications
    pub fn handle_inventory_event(&self, _notification: InventoryEvent) {
        println!("Solver: Handle Inventory Event");
        //self.solve();
    }

    /// receive current prices from Price Tracker
    pub fn handle_price_event(&self, _notification: PriceEvent) {
        println!("Solver: Handle Price Event");
        //self.solve();
    }

    /// receive available liquidity from Order Book Manager
    pub fn handle_book_event(&self, _notification: OrderBookEvent) {
        println!("Solver: Handle Book Event");
        //self.solve();
    }

    /// receive basket notification
    pub fn handle_basket_event(&self, notification: BasketNotification) {
        println!("Solver: Handle Basket Notification");
        //self.solve();
        // TODO: (move this) once solvign is done notify new weights were applied
        match notification {
            BasketNotification::BasketAdded(symbol, basket) => self
                .chain_connector
                .write()
                .solver_weights_set(symbol, basket),
            BasketNotification::BasketUpdated(symbol, basket) => self
                .chain_connector
                .write()
                .solver_weights_set(symbol, basket),
            BasketNotification::BasketRemoved(_symbol) => todo!(),
        }
    }
}

#[cfg(test)]
mod test {
    use std::{any::type_name, sync::Arc, time::Duration};

    use crossbeam::{
        channel::{unbounded, Sender},
        select,
    };

    use crate::{
        blockchain::chain_connector::test_util::{
            MockChainConnector, MockChainInternalNotification,
        },
        core::{
            bits::PricePointEntry,
            functional::{
                IntoNotificationHandlerOnceBox, IntoObservableMany, IntoObservableSingle,
                NotificationHandlerOnce,
            },
            test_util::{
                get_mock_asset_1_arc, get_mock_asset_2_arc, get_mock_asset_name_1,
                get_mock_asset_name_2, get_mock_decimal, get_mock_index_name_1,
            },
        },
        index::{
            basket::{AssetWeight, BasketDefinition},
            basket_manager::BasketNotification,
        },
        market_data::{
            market_data_connector::{
                test_util::MockMarketDataConnector, MarketDataConnector, MarketDataEvent,
            },
            order_book::order_book_manager::PricePointBookManager,
        },
        order_sender::{
            order_connector::{test_util::MockOrderConnector, OrderConnectorNotification},
            order_tracker::{OrderTracker, OrderTrackerNotification},
        },
        server::server::{test_util::MockServer, ServerEvent},
        solver::index_quote_manager::test_util::MockQuoteRequestManager,
    };

    use super::*;

    impl<T> NotificationHandlerOnce<T> for Sender<T>
    where
        T: Send + Sync,
    {
        fn handle_notification(&self, notification: T) {
            self.send(notification)
                .expect(format!("Failed to handle {}", type_name::<T>()).as_str());
        }
    }

    impl<T> IntoNotificationHandlerOnceBox<T> for Sender<T>
    where
        T: Send + Sync + 'static,
    {
        fn into_notification_handler_once_box(self) -> Box<dyn NotificationHandlerOnce<T>> {
            Box::new(self)
        }
    }

    #[test]
    fn sbe_solver() {
        let tolerance = get_mock_decimal("0.0001");

        let (chain_sender, chain_receiver) = unbounded::<ChainNotification>();
        let (index_order_sender, index_order_receiver) = unbounded::<IndexOrderEvent>();
        let (quote_request_sender, quote_request_receiver) = unbounded::<QuoteRequestEvent>();
        let (inventory_sender, inventory_receiver) = unbounded::<InventoryEvent>();
        let (book_sender, book_receiver) = unbounded::<OrderBookEvent>();
        let (price_sender, price_receiver) = unbounded::<PriceEvent>();
        let (market_sender, market_receiver) = unbounded::<Arc<MarketDataEvent>>();
        let (basket_sender, basket_receiver) = unbounded::<BasketNotification>();
        let (fix_server_sender, fix_server_receiver) = unbounded::<Arc<ServerEvent>>();
        let (order_tracker_sender, order_tracker_receiver) =
            unbounded::<OrderTrackerNotification>();
        let (order_connector_sender, order_connector_receiver) =
            unbounded::<OrderConnectorNotification>();

        /*
        NOTES:
        This SBE is to demonstrate general structure of the application.
        We can see dependencies (direct ownership), as well as dependency inversions (events).
        In this example we use direct callbacks from event source to event handler.
        The production version will make use of channels, and dispatch, but we need to
        be careful to ensure FIFO event ordering.
        */
        let order_connector = Arc::new(RwLock::new(MockOrderConnector::new()));
        let order_tracker = Arc::new(RwLock::new(OrderTracker::new(
            order_connector.clone(),
            tolerance,
        )));
        let inventory_manager = Arc::new(RwLock::new(InventoryManager::new(
            order_tracker.clone(),
            tolerance,
        )));

        let market_data_connector = Arc::new(RwLock::new(MockMarketDataConnector::new()));
        let order_book_manager = Arc::new(RwLock::new(PricePointBookManager::new(tolerance)));
        let price_tracker = Arc::new(RwLock::new(PriceTracker::new()));

        let chain_connector = Arc::new(RwLock::new(MockChainConnector::new()));
        let fix_server = Arc::new(RwLock::new(MockServer::new()));

        let index_order_manager = Arc::new(RwLock::new(IndexOrderManager::new(
            fix_server.clone(),
            tolerance,
        )));
        let quote_request_manager = Arc::new(RwLock::new(MockQuoteRequestManager::new(
            fix_server.clone(),
        )));

        let basket_manager = Arc::new(RwLock::new(BasketManager::new()));

        let solver = Arc::new(Solver::new(
            chain_connector.clone(),
            index_order_manager.clone(),
            quote_request_manager.clone(),
            basket_manager.clone(),
            price_tracker.clone(),
            order_book_manager.clone(),
            inventory_manager.clone(),
        ));

        solver
            .basket_manager
            .write()
            .get_single_observer_mut()
            .set_observer_from(basket_sender);

        chain_connector
            .write()
            .get_single_observer_mut()
            .set_observer_from(chain_sender);

        index_order_manager
            .write()
            .get_single_observer_mut()
            .set_observer_from(index_order_sender);

        quote_request_manager
            .write()
            .get_single_observer_mut()
            .set_observer_from(quote_request_sender);

        inventory_manager
            .write()
            .get_single_observer_mut()
            .set_observer_from(inventory_sender);

        price_tracker
            .write()
            .get_single_observer_mut()
            .set_observer_from(price_sender);

        order_book_manager
            .write()
            .get_single_observer_mut()
            .set_observer_from(book_sender);

        market_data_connector
            .write()
            .get_multi_observer_mut()
            .add_observer_fn(move |e: &Arc<MarketDataEvent>| {
                market_sender.send(e.clone()).unwrap()
            });

        fix_server
            .write()
            .get_multi_observer_mut()
            .add_observer_fn(move |e: &Arc<ServerEvent>| {
                fix_server_sender.send(e.clone()).unwrap();
            });

        order_tracker
            .write()
            .get_single_observer_mut()
            .set_observer_from(order_tracker_sender);

        order_connector
            .write()
            .get_single_observer_mut()
            .set_observer_fn(order_connector_sender);

        let flush_events = move || {
            // Simple dispatch loop
            loop {
                select! {
                    recv(chain_receiver) -> res => solver.handle_chain_event(res.unwrap()),
                    recv(index_order_receiver) -> res => solver.handle_index_order(res.unwrap()),
                    recv(quote_request_receiver) -> res => solver.handle_quote_request(res.unwrap()),
                    recv(inventory_receiver) -> res => solver.handle_inventory_event(res.unwrap()),
                    recv(price_receiver) -> res => solver.handle_price_event(res.unwrap()),
                    recv(book_receiver) -> res => solver.handle_book_event(res.unwrap()),
                    recv(basket_receiver) -> res => solver.handle_basket_event(res.unwrap()),

                    recv(market_receiver) -> res => {
                        let e = res.unwrap();
                        price_tracker
                            .write()
                            .handle_market_data(&e);

                        order_book_manager
                            .write()
                            .handle_market_data(&e);
                    },
                    recv(fix_server_receiver) -> res => {
                        let e = res.unwrap();
                        index_order_manager
                            .write()
                            .handle_server_message(&e)
                            .expect("Failed to handle index order");

                        quote_request_manager
                            .write()
                            .handle_server_message(&e);
                    },
                    recv(order_tracker_receiver) -> res => {
                        inventory_manager
                            .write()
                            .handle_fill_report(res.unwrap())
                            .expect("Failed to handle fill report");
                    },
                    recv(order_connector_receiver) -> res => {
                        order_tracker
                            .write()
                            .handle_order_notification(res.unwrap());
                    },
                    default => { break; },
                }
            }
        };

        let (mock_chain_sender, mock_chain_receiver) = unbounded::<MockChainInternalNotification>();
        chain_connector
            .write()
            .internal_observer
            .set_observer_from(mock_chain_sender);

        // connect to exchange
        order_connector.write().connect();

        // connect to exchange
        market_data_connector.write().connect();

        // subscribe to symbol/USDC markets
        market_data_connector
            .write()
            .subscribe(&[get_mock_asset_name_1(), get_mock_asset_name_2()])
            .unwrap();

        // send some market data
        // top of the book
        market_data_connector.write().notify_top_of_book(
            get_mock_asset_name_1(),
            get_mock_decimal("90.0"),
            get_mock_decimal("100.0"),
            get_mock_decimal("10.0"),
            get_mock_decimal("20.0"),
        );

        market_data_connector.write().notify_top_of_book(
            get_mock_asset_name_2(),
            get_mock_decimal("295.0"),
            get_mock_decimal("300.0"),
            get_mock_decimal("80.0"),
            get_mock_decimal("50.0"),
        );

        // last trade
        market_data_connector.write().notify_trade(
            get_mock_asset_name_1(),
            get_mock_decimal("90.0"),
            get_mock_decimal("5.0"),
        );

        market_data_connector.write().notify_trade(
            get_mock_asset_name_2(),
            get_mock_decimal("300.0"),
            get_mock_decimal("15.0"),
        );

        // book depth
        market_data_connector.write().notify_full_order_book(
            get_mock_asset_name_1(),
            vec![
                PricePointEntry {
                    price: get_mock_decimal("90.0"),
                    quantity: get_mock_decimal("10.0"),
                },
                PricePointEntry {
                    price: get_mock_decimal("80.0"),
                    quantity: get_mock_decimal("40.0"),
                },
            ],
            vec![
                PricePointEntry {
                    price: get_mock_decimal("100.0"),
                    quantity: get_mock_decimal("20.0"),
                },
                PricePointEntry {
                    price: get_mock_decimal("110.0"),
                    quantity: get_mock_decimal("30.0"),
                },
            ],
        );

        market_data_connector.write().notify_full_order_book(
            get_mock_asset_name_2(),
            vec![
                PricePointEntry {
                    price: get_mock_decimal("295.0"),
                    quantity: get_mock_decimal("80.0"),
                },
                PricePointEntry {
                    price: get_mock_decimal("290.0"),
                    quantity: get_mock_decimal("100.0"),
                },
            ],
            vec![
                PricePointEntry {
                    price: get_mock_decimal("300.0"),
                    quantity: get_mock_decimal("50.0"),
                },
                PricePointEntry {
                    price: get_mock_decimal("305.0"),
                    quantity: get_mock_decimal("150.0"),
                },
            ],
        );

        // necessary to wait until all market data events are ingested
        flush_events();

        // define basket
        let basket_definition = BasketDefinition::try_new(vec![
            AssetWeight::new(get_mock_asset_1_arc(), get_mock_decimal("0.25")),
            AssetWeight::new(get_mock_asset_2_arc(), get_mock_decimal("0.75")),
        ])
        .unwrap();

        // send basket weights
        chain_connector
            .write()
            .notify_curator_weights_set(get_mock_index_name_1(), basket_definition);

        flush_events();

        // wait for solver to solve...
        let solver_weithgs_set = mock_chain_receiver
            .recv_timeout(Duration::from_secs(1))
            .expect("Failed to receive SolverWeightsSet");

        assert!(matches!(solver_weithgs_set, MockChainInternalNotification::SolverWeightsSet(_, _)));

        //fix_server.write().notify_fix_message(());
    }
}
