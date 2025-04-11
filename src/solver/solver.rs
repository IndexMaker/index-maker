use std::sync::Arc;

use parking_lot::RwLock;

use crate::{
    blockchain::chain_connector::{ChainConnector, ChainNotification},
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
pub trait Solver {}
pub struct MockSolver {
    pub chain_connector: Arc<RwLock<dyn ChainConnector>>,
    pub index_order_manager: Arc<RwLock<dyn IndexOrderManager>>,
    pub quote_request_manager: Arc<RwLock<dyn QuoteRequestManager>>,
    pub basket_manager: Arc<RwLock<BasketManager>>,
    pub price_tracker: Arc<RwLock<dyn PriceTracker>>,
    pub order_book_manager: Arc<RwLock<dyn OrderBookManager>>,
    pub inventory_manager: Arc<RwLock<dyn InventoryManager>>,
}
impl MockSolver {
    pub fn new(
        chain_connector: Arc<RwLock<dyn ChainConnector>>,
        index_order_manager: Arc<RwLock<dyn IndexOrderManager>>,
        quote_request_manager: Arc<RwLock<dyn QuoteRequestManager>>,
        basket_manager: Arc<RwLock<BasketManager>>,
        price_tracker: Arc<RwLock<dyn PriceTracker>>,
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

    pub fn handle_chain_event(&self, _notification: ChainNotification) {
        todo!()
    }

    pub fn handle_index_order(&self, _notification: IndexOrderEvent) {
        todo!()
    }

    pub fn handle_quote_request(&self, _notification: QuoteRequestEvent) {
        todo!()
    }

    pub fn handle_inventory_event(&self, _notification: InventoryEvent) {
        todo!()
    }

    pub fn handle_price_event(&self, _notification: PriceEvent) {
        todo!()
    }

    pub fn handle_book_event(&self, _notification: OrderBookEvent) {
        todo!()
    }
}

impl Solver for MockSolver {}

#[cfg(test)]
mod test {
    use std::sync::Arc;

    use crate::{
        blockchain::chain_connector::test_util::MockChainConnector,
        market_data::{
            market_data_connector::{test_util::MockMarketDataConnector, MarketDataEvent},
            order_book::order_book_manager::test_util::MockOrderBookManager,
            price_tracker::test_util::MockPriceTracker,
        },
        order_sender::{
            order_connector::test_util::MockOrderConnector,
            order_tracker::test_util::MockOrderTracker,
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
        /*
        NOTES:
        This SBE is to demonstrate general structure of the application.
        We can see dependencies (direct ownership), as well as dependency inversions (events).
        In this example we use direct callbacks from event source to event handler.
        The production version will make use of channels, and dispatch, but we need to
        be careful to ensure FIFO event ordering.
        */
        let order_connector = Arc::new(MockOrderConnector::new());
        let order_tracker = Arc::new(MockOrderTracker::new(order_connector));
        let inventory_manager = Arc::new(RwLock::new(MockInventoryManager::new(order_tracker)));

        let market_data_connector = Arc::new(RwLock::new(MockMarketDataConnector::new()));
        let order_book_manager = Arc::new(RwLock::new(MockOrderBookManager::new(
            market_data_connector.clone(),
        )));
        let price_tracker = Arc::new(RwLock::new(MockPriceTracker::new(
            market_data_connector.clone(),
        )));

        let chain_connector = Arc::new(RwLock::new(MockChainConnector::new()));
        let fix_server = Arc::new(RwLock::new(MockServer::new()));

        let index_order_manager =
            Arc::new(RwLock::new(MockIndexOrderManager::new(fix_server.clone())));
        let quote_request_manager = Arc::new(RwLock::new(MockQuoteRequestManager::new(
            fix_server.clone(),
        )));

        let basket_manager = Arc::new(RwLock::new(BasketManager::new()));

        let solver = Arc::new(MockSolver::new(
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
    }
}
