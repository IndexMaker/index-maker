use std::sync::{Arc, Weak};

use parking_lot::RwLock;

use crate::core::bits::{Amount, Symbol};

/// magic solver, needs to take index orders, and based on prices (from price
/// tracker) and available liquiduty (depth from order books), and active orders
/// (from order tracker) calculate best internal-portfolio rebalancing orders,
/// which will (partly) fill (some of the) ordered indexes.  Any position that
/// wasn't matched against ordered indexes shouldn't be kept for too long.

pub enum ChainEvent {
    CuratorWeightsSet{curator: Symbol, weights: (), timestamp: ()}
}
pub trait ChainEventObserver {
    fn handle_chain_event(&self, event: ChainEvent);
}
pub trait ChainConnector {
    fn add_observer(&mut self, observer: Weak<dyn ChainEventObserver>);
}
pub struct MockChainConnector {
    observer: Option<Weak<dyn Solver>>
}
impl MockChainConnector {
    pub fn new() -> Self {
        Self {
            observer: None
        }
    }
}
impl ChainConnector for MockChainConnector {
    fn add_observer(&mut self, _observer: Weak<dyn ChainEventObserver>) {
        todo!()
    }
}

pub enum MarketDataEvent {
    TopOfBook{symbol: Symbol, bid: Amount, ask: Amount, bid_quantity: Amount, ask_quantity: Amount},
    Trade{symbol: Symbol, price: Amount, quantity: Amount},
    FullOrderBook{symbol: Symbol, price_levels: ()}
}
pub trait MarketDataObserver {
    fn handle_market_data(&self, event: MarketDataEvent);
}
pub trait MarketDataConnector {
    fn add_observer(&mut self, observer: Weak<dyn MarketDataObserver>);
}
pub struct MockMarketDataConnector {}
impl MockMarketDataConnector {
    pub fn new() -> Self {
        Self {}
    }
}
impl MarketDataConnector for MockMarketDataConnector {
     fn add_observer(&mut self, _observer: Weak<dyn MarketDataObserver>) {
         todo!()
     }
}

pub trait OrderConnector {}
pub struct MockOrderConnector {}
impl MockOrderConnector {
    pub fn new() -> Self {
        Self {}
    }
}
impl OrderConnector for MockOrderConnector {}

pub enum IndexOrderEvent {
    NewIndexOrder{symbol: Symbol, price: Amount, quantity: Amount, side: (), client_order_id: ()},
    CancelIndexOrder{client_order_id: ()}
}
pub trait IndexOrderObserver {
    fn handle_index_order(&self, event: IndexOrderEvent);
}
pub trait IndexOrderManager {
    fn add_observer(&mut self, observer: Weak<dyn IndexOrderObserver>);
}
pub struct MockIndexOrderManager {
    server: Arc<RwLock<dyn Server>>
}
impl MockIndexOrderManager {
    pub fn new(
        server: Arc<RwLock<dyn Server>>
    ) -> Self {
        Self {
            server
        }
    }
}
impl IndexOrderManager for MockIndexOrderManager {
    fn add_observer(&mut self, _observer: Weak<dyn IndexOrderObserver>) {
        todo!()
    }
}
impl ServerEventObserver for RwLock<MockIndexOrderManager> {
    fn handle_server_event(&self, _event: ServerEvent) {
        todo!()
    }
}

pub enum QuoteRequestEvent {
    NewQuoteRequest,
    CancelQuoteRequest
}
pub trait QuoteRequestObserver {
    fn handle_quote_request(&self, event: QuoteRequestEvent);
}
pub trait QuoteRequestManager {
    fn add_observer(&mut self, observer: Weak<dyn QuoteRequestObserver>);
}
pub struct MockQuoteRequestManager {
    server: Arc<RwLock<dyn Server>>
}
impl MockQuoteRequestManager {
    pub fn new(
        server: Arc<RwLock<dyn Server>>
    ) -> Self {
        Self {
            server
        }
    }
}
impl QuoteRequestManager for MockQuoteRequestManager {
    fn add_observer(&mut self, _observer: Weak<dyn QuoteRequestObserver>) {
        todo!()
    }
}
impl ServerEventObserver for RwLock<MockQuoteRequestManager> {
    fn handle_server_event(&self, event: ServerEvent) {
        todo!()
    }
}

pub trait BasketManager {}
pub struct MockBasketManager {}
impl MockBasketManager {
    pub fn new() -> Self {
        Self {}
    }
}
impl BasketManager for MockBasketManager {}

pub trait OrderTracker {}
pub struct MockOrderTracker {
    order_connector: Arc<dyn OrderConnector>
}
impl MockOrderTracker {
    pub fn new(order_connector: Arc<dyn OrderConnector>) -> Self {
        Self {
            order_connector
        }
    }
}
impl OrderTracker for MockOrderTracker {}


pub trait PriceTracker {}
pub struct MockPriceTracker {
    market_data_connector: Arc<RwLock<dyn MarketDataConnector>>
}
impl MockPriceTracker {
    pub fn new(
        market_data_connector: Arc<RwLock<dyn MarketDataConnector>>
    ) -> Self {
        Self {
            market_data_connector
        }
    }
}
impl PriceTracker for MockPriceTracker {}
impl MarketDataObserver for MockPriceTracker {
    fn handle_market_data(&self, event: MarketDataEvent) {
        todo!()
    }
}


pub trait OrderBookManager {}
pub struct MockOrderBookManager {
    market_data_connector: Arc<RwLock<dyn MarketDataConnector>>
}
impl MockOrderBookManager {
    pub fn new(market_data_connector: Arc<RwLock<dyn MarketDataConnector>>) -> Self {
        Self {
            market_data_connector
        }
    }
}
impl OrderBookManager for MockOrderBookManager {}
impl MarketDataObserver for MockOrderBookManager {
    fn handle_market_data(&self, _event: MarketDataEvent) {
        todo!()
    }
}


pub trait InventoryManager {}
pub struct MockInventoryManager {
    order_tracker: Arc<dyn OrderTracker>
}
impl MockInventoryManager {
    pub fn new(order_tracker: Arc<dyn OrderTracker>) -> Self {
        Self {
            order_tracker
        }
    }
}
impl InventoryManager for MockInventoryManager {}


pub trait Solver {}
pub struct MockSolver {
    chain_connector: Arc<RwLock<dyn ChainConnector>>,
    index_order_manager: Arc<RwLock<dyn IndexOrderManager>>,
    quote_request_manager: Arc<RwLock<dyn QuoteRequestManager>>,
    basket_manager: Arc<dyn BasketManager>,
    price_tracker: Arc<dyn PriceTracker>,
    order_book_manager: Arc<dyn OrderBookManager>,
    inventory_manager: Arc<dyn InventoryManager>
}
impl MockSolver {
    pub fn new(
        chain_connector: Arc<RwLock<dyn ChainConnector>>,
        index_order_manager: Arc<RwLock<dyn IndexOrderManager>>,
        quote_request_manager: Arc<RwLock<dyn QuoteRequestManager>>,
        basket_manager: Arc<dyn BasketManager>,
        price_tracker: Arc<dyn PriceTracker>,
        order_book_manager: Arc<dyn OrderBookManager>,
        inventory_manager: Arc<dyn InventoryManager>
    ) -> Self {
        Self {
            chain_connector,
            index_order_manager,
            quote_request_manager,
            basket_manager,
            price_tracker,
            order_book_manager,
            inventory_manager
        }
    }
}
impl Solver for MockSolver {}
impl ChainEventObserver for MockSolver {
    fn handle_chain_event(&self, _event: ChainEvent) {
        todo!()
    }
}
impl IndexOrderObserver for MockSolver {
    fn handle_index_order(&self, _event: IndexOrderEvent) {
        todo!()
    }
}
impl QuoteRequestObserver for MockSolver {
    fn handle_quote_request(&self, _event: QuoteRequestEvent) {
        todo!()
    }
}

pub enum ServerEvent {
    NewIndexOrder,
    NewQuoteRequest,
    CancelIndexOrder,
    CancelQuoteRequest,
    AccountToCustody,
    CustodyToAccount
}
pub trait ServerEventObserver {
    fn handle_server_event(&self, event: ServerEvent);
}
pub trait Server {
    fn add_observer(&mut self, observer: Weak<dyn ServerEventObserver>);
}
pub struct MockServer { }
impl MockServer {
    pub fn new() -> Self {
        Self {}
    }
}
impl Server for MockServer {
    fn add_observer(&mut self, _observer: Weak<dyn ServerEventObserver>) {
        todo!()
    }
}


#[cfg(test)]
mod test {
    use std::sync::Arc;

    use super::*;


    #[test]
    #[ignore = "Not implemented yet. Only SBE design."]
    fn sbe_solver() {
        let order_connector = Arc::new(MockOrderConnector::new());
        let order_tracker = Arc::new(MockOrderTracker::new(order_connector));
        let inventory_manager = Arc::new(MockInventoryManager::new(order_tracker));

        let market_data_connector = Arc::new(RwLock::new(MockMarketDataConnector::new()));
        let order_book_manager = Arc::new(MockOrderBookManager::new(market_data_connector.clone()));
        let price_tracker = Arc::new(MockPriceTracker::new(market_data_connector.clone()));

        let chain_connector = Arc::new(RwLock::new(MockChainConnector::new()));
        let fix_server = Arc::new(RwLock::new(MockServer::new()));
        
        let index_order_manager = Arc::new(RwLock::new(MockIndexOrderManager::new(fix_server.clone())));
        let quote_request_manager = Arc::new(RwLock::new(MockQuoteRequestManager::new(fix_server.clone())));

        let basket_manager = Arc::new(MockBasketManager::new());

        let solver = Arc::new(MockSolver::new(
            chain_connector.clone(),
            index_order_manager.clone(),
            quote_request_manager.clone(),
            basket_manager.clone(),
            price_tracker.clone(),
            order_book_manager.clone(),
            inventory_manager.clone()));

        // Solver internally will
        // 1. To receive events from chain
        chain_connector.write().add_observer(Arc::downgrade(&(solver.clone() as Arc<dyn ChainEventObserver>)));
        // 2. To receive index orders from endpoint (FIX, REST, WS, ...)
        index_order_manager.write().add_observer(Arc::downgrade(&(solver.clone() as Arc<dyn IndexOrderObserver>)));
        // 3. To receive index quote request from endpoint (FIX, REST, WS, ...)
        quote_request_manager.write().add_observer(Arc::downgrade(&(solver.clone() as Arc<dyn QuoteRequestObserver>)));

        // OrderBookManager internally will
        // because it will use the connector to subscribe for market data, and
        // it will register itself as market data observer
        market_data_connector.write().add_observer(
            Arc::downgrade(&(order_book_manager as Arc<dyn MarketDataObserver>))
        );

        // PriceTracker internally will
        // because it will use the connector to subscribe for market data, and
        // it will register itself as market data observer
        market_data_connector.write().add_observer(
            Arc::downgrade(&(price_tracker as Arc<dyn MarketDataObserver>))
        );

        // IndexOrderManager internally will
        fix_server.write().add_observer(
            Arc::downgrade(&(index_order_manager as Arc<dyn ServerEventObserver>))
        );

        // QuoteRequestManager internally will
        fix_server.write().add_observer(
            Arc::downgrade(&(quote_request_manager as Arc<dyn ServerEventObserver>))
        );
    }

}