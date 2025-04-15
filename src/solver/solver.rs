use std::{collections::HashMap, sync::Arc};

use parking_lot::RwLock;

use crate::{
    blockchain::chain_connector::{ChainConnector, ChainNotification},
    core::bits::{Amount, ClientOrderId, PriceType, Side, Symbol},
    index::basket_manager::BasketManager,
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
    pub chain_connector: Arc<RwLock<dyn ChainConnector>>,
    pub index_order_manager: Arc<RwLock<dyn IndexOrderManager>>,
    pub quote_request_manager: Arc<RwLock<dyn QuoteRequestManager>>,
    pub basket_manager: Arc<RwLock<BasketManager>>,
    pub price_tracker: Arc<RwLock<PriceTracker>>,
    pub order_book_manager: Arc<RwLock<dyn OrderBookManager>>,
    pub inventory_manager: Arc<RwLock<dyn InventoryManager>>,
}
impl Solver {
    pub fn new(
        chain_connector: Arc<RwLock<dyn ChainConnector>>,
        index_order_manager: Arc<RwLock<dyn IndexOrderManager>>,
        quote_request_manager: Arc<RwLock<dyn QuoteRequestManager>>,
        basket_manager: Arc<RwLock<BasketManager>>,
        price_tracker: Arc<RwLock<PriceTracker>>,
        order_book_manager: Arc<RwLock<dyn OrderBookManager>>,
        inventory_manager: Arc<RwLock<dyn InventoryManager>>,
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
        let _open_lots = self.inventory_manager.read().get_open_lots(&symbols);

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
        self.inventory_manager.write().new_order(());
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
        match notification {
            ChainNotification::CuratorWeightsSet(basket_definition) => {
                if let Err(_) = self.basket_manager.write().set_basket_from_definition(
                    Symbol::default(), // <- name of an Index
                    basket_definition,
                    &HashMap::new(),   // <- get current prices from price tracker
                    Amount::default(), // <- calculate target price
                ) {
                    todo!("Implement error logging")
                } else {
                    // Recalculate position and send adequate orders
                    self.solve();
                }
            }
        }
    }

    /// receive Index Order
    pub fn handle_index_order(&self, _notification: IndexOrderEvent) {
        self.solve();
    }

    // receive QR
    pub fn handle_quote_request(&self, _notification: QuoteRequestEvent) {
        self.quote(());
    }

    /// Receive fill notifications
    pub fn handle_inventory_event(&self, _notification: InventoryEvent) {
        self.solve();
    }

    /// receive current prices from Price Tracker
    pub fn handle_price_event(&self, _notification: PriceEvent) {
        self.solve();
    }

    /// receive available liquidity from Order Book Manager
    pub fn handle_book_event(&self, _notification: OrderBookEvent) {
        self.solve();
    }
}

#[cfg(test)]
mod test {
    use std::sync::Arc;

    use crate::{
        blockchain::chain_connector::test_util::MockChainConnector,
        core::{bits::Symbol, test_util::get_mock_decimal},
        market_data::{
            market_data_connector::{
                test_util::MockMarketDataConnector, MarketDataConnector, MarketDataEvent,
            },
            order_book::order_book_manager::PricePointBookManager,
        },
        order_sender::{
            order_connector::test_util::MockOrderConnector, order_tracker::OrderTracker,
        },
        server::server::{test_util::MockServer, ServerEvent},
        solver::{
            index_order_manager::test_util::MockIndexOrderManager,
            index_quote_manager::test_util::MockQuoteRequestManager,
            inventory_manager::test_util::MockInventoryManager,
        },
    };

    use super::*;

    #[test]
    #[ignore = "Not implemented yet. Only SBE design."]
    fn sbe_solver() {
        let tolerance = get_mock_decimal("0.0001");
        /*
        NOTES:
        This SBE is to demonstrate general structure of the application.
        We can see dependencies (direct ownership), as well as dependency inversions (events).
        In this example we use direct callbacks from event source to event handler.
        The production version will make use of channels, and dispatch, but we need to
        be careful to ensure FIFO event ordering.
        */
        let order_connector = Arc::new(RwLock::new(MockOrderConnector::new()));
        let order_tracker = Arc::new(RwLock::new(OrderTracker::new(order_connector.clone(), tolerance)));
        let inventory_manager = Arc::new(RwLock::new(MockInventoryManager::new(
            order_tracker.clone(),
        )));

        let market_data_connector = Arc::new(RwLock::new(MockMarketDataConnector::new()));
        let order_book_manager = Arc::new(RwLock::new(PricePointBookManager::new(tolerance)));
        let price_tracker = Arc::new(RwLock::new(PriceTracker::new()));

        let chain_connector = Arc::new(RwLock::new(MockChainConnector::new()));
        let fix_server = Arc::new(RwLock::new(MockServer::new()));

        let index_order_manager =
            Arc::new(RwLock::new(MockIndexOrderManager::new(fix_server.clone())));
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

        let solver_weak_1 = Arc::downgrade(&solver);
        let solver_weak_2 = solver_weak_1.clone();
        let solver_weak_3 = solver_weak_1.clone();
        let solver_weak_4 = solver_weak_1.clone();
        let solver_weak_5 = solver_weak_1.clone();
        let solver_weak_6 = solver_weak_1.clone();

        // Solver internally will
        // 1. To receive events from chain
        chain_connector
            .write()
            .observer
            .set_observer_fn(move |e| solver_weak_1.upgrade().unwrap().handle_chain_event(e));

        // 2. To receive index orders from endpoint (FIX, REST, WS, ...)
        index_order_manager
            .write()
            .observer
            .set_observer_fn(move |e| solver_weak_2.upgrade().unwrap().handle_index_order(e));

        // 3. To receive index quote request from endpoint (FIX, REST, WS, ...)
        quote_request_manager
            .write()
            .observer
            .set_observer_fn(move |e| solver_weak_3.upgrade().unwrap().handle_quote_request(e));

        inventory_manager
            .write()
            .observer
            .set_observer_fn(move |e| solver_weak_4.upgrade().unwrap().handle_inventory_event(e));

        price_tracker
            .write()
            .observer
            .set_observer_fn(move |e| solver_weak_5.upgrade().unwrap().handle_price_event(e));

        order_book_manager
            .write()
            .observer
            .set_observer_fn(move |e| solver_weak_6.upgrade().unwrap().handle_book_event(e));

        let order_book_manager_weak = Arc::downgrade(&order_book_manager);
        let price_tracker_weak = Arc::downgrade(&price_tracker);

        // OrderBookManager internally will
        // because it will use the connector to subscribe for market data, and
        // it will register itself as market data observer
        market_data_connector
            .write()
            .observer
            .add_observer_fn(move |e: &Arc<MarketDataEvent>| {
                order_book_manager_weak
                    .upgrade()
                    .unwrap()
                    .write()
                    .handle_market_data(e)
            });

        // PriceTracker internally will
        // because it will use the connector to subscribe for market data, and
        // it will register itself as market data observer
        market_data_connector
            .write()
            .observer
            .add_observer_fn(move |e: &Arc<MarketDataEvent>| {
                price_tracker_weak
                    .upgrade()
                    .unwrap()
                    .write()
                    .handle_market_data(e)
            });

        let index_order_manager_weak = Arc::downgrade(&index_order_manager);
        let quote_request_manager_weak = Arc::downgrade(&quote_request_manager);

        // IndexOrderManager internally will
        fix_server
            .write()
            .observers
            .add_observer_fn(move |e: &Arc<ServerEvent>| {
                index_order_manager_weak
                    .upgrade()
                    .unwrap()
                    .write()
                    .handle_server_message(e)
            });

        // QuoteRequestManager internally will
        fix_server
            .write()
            .observers
            .add_observer_fn(move |e: &Arc<ServerEvent>| {
                quote_request_manager_weak
                    .upgrade()
                    .unwrap()
                    .write()
                    .handle_server_message(e)
            });

        let inventory_manager_weak = Arc::downgrade(&inventory_manager);

        order_tracker.write().observer.set_observer_fn(move |e| {
            inventory_manager_weak
                .upgrade()
                .unwrap()
                .write()
                .handle_fill_report(e)
        });

        let order_tracker_weak = Arc::downgrade(&order_tracker);

        order_connector.write().observer.set_observer_fn(move |e| {
            order_tracker_weak
                .upgrade()
                .unwrap()
                .write()
                .handle_order_notification(e);
        });

        // connect to exchange
        order_connector.write().connect();

        // connect to exchange
        market_data_connector.write().connect();

        // subscribe to symbol/USDC markets
        market_data_connector
            .write()
            .subscribe(&[Symbol::default()])
            .unwrap();
    }
}
