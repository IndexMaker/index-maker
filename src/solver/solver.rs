use std::{
    collections::{hash_map::Entry, HashMap, VecDeque},
    sync::Arc,
};

use chrono::{DateTime, TimeDelta, Utc};
use eyre::{eyre, OptionExt, Result};
use itertools::Itertools;
use parking_lot::{Mutex, RwLock};
use safe_math::safe;

use crate::{
    blockchain::chain_connector::{ChainConnector, ChainNotification},
    core::{
        bits::{
            Address, Amount, BatchOrder, BatchOrderId, ClientOrderId, OrderId, PaymentId,
            PriceType, Side, Symbol,
        },
        decimal_ext::DecimalExt,
    },
    index::{
        basket::Basket,
        basket_manager::{BasketManager, BasketNotification},
    },
    market_data::{
        order_book::order_book_manager::{OrderBookEvent, OrderBookManager},
        price_tracker::{GetPricesResponse, PriceEvent, PriceTracker},
    },
};

use super::{
    batch_manager::{BatchManager, BatchManagerHost},
    collateral_manager::{CollateralManager, CollateralManagerHost},
    index_order_manager::{EngageOrderRequest, IndexOrderEvent, IndexOrderManager},
    index_quote_manager::{QuoteRequestEvent, QuoteRequestManager},
    inventory_manager::{InventoryEvent, InventoryManager},
};

#[derive(Clone, Copy, Debug)]
pub enum SolverOrderStatus {
    Open,
    Ready,
    Engaged,
    PartlyMintable,
    FullyMintable,
    Minted,
    MissingPrices,
    InvalidSymbol,
    MathOverflow,
}

/// Solver's view of the Index Order
pub struct SolverOrder {
    // ID of the original NewOrder request
    pub original_client_order_id: ClientOrderId,

    /// On-chain address of the User
    pub address: Address,

    // ID of the NewOrder request
    pub client_order_id: ClientOrderId,

    /// An ID of the on-chain payment
    pub payment_id: PaymentId,

    /// Symbol of an Index
    pub symbol: Symbol,

    /// Side of an order
    pub side: Side,

    /// Collateral remaining on the order to complete
    pub remaining_collateral: Amount,

    /// Collateral solver has engaged so far
    pub engaged_collateral: Amount,

    /// Collateral carried over from last batch
    pub collateral_carried: Amount,

    /// Collateral spent by solver
    pub collateral_spent: Amount,

    /// Quantity filled by subsequent batch order fills
    pub filled_quantity: Amount,

    /// Time when requested
    pub timestamp: DateTime<Utc>,

    /// Solver status
    pub status: SolverOrderStatus,

    /// All asset lots allocated to this Index Order
    pub lots: Vec<SolverOrderAssetLot>,
}

/// When we fill Index Orders from executed batches, we need to allocate some
/// portion of what was executed to each index order in the batch using
/// contribution factor.
pub struct SolverOrderAssetLot {
    /// Symbol of an asset
    pub symbol: Symbol,

    /// Quantity allocated to index order
    pub quantity: Amount,

    /// Executed price
    pub price: Amount,

    /// Execution fee
    pub fee: Amount,
}

impl SolverOrderAssetLot {
    pub fn compute_collateral_spent(&self) -> Option<Amount> {
        safe!(safe!(self.quantity * self.price) + self.fee)
    }
}

pub struct SolverOrderEngagement {
    pub index_order: Arc<RwLock<SolverOrder>>,
    pub asset_contribution_fractions: HashMap<Symbol, Amount>,
    pub asset_quantities: HashMap<Symbol, Amount>,
    pub asset_price_limits: HashMap<Symbol, Amount>,
    pub address: Address,
    pub client_order_id: ClientOrderId,
    pub symbol: Symbol,
    pub basket: Arc<Basket>,
    pub engaged_side: Side,
    pub engaged_collateral: Amount,
    pub engaged_quantity: Amount,
    pub engaged_price: Amount,
    pub filled_quantity: Amount,
}

pub struct EngagedSolverOrders {
    pub batch_order_id: BatchOrderId,
    pub engaged_orders: Vec<RwLock<SolverOrderEngagement>>,
}

pub trait OrderIdProvider {
    fn next_order_id(&mut self) -> OrderId;
    fn next_batch_order_id(&mut self) -> BatchOrderId;
}

pub trait SetSolverOrderStatus {
    fn set_order_status(&self, order: &mut SolverOrder, status: SolverOrderStatus);
}

pub trait SolverStrategyHost: SetSolverOrderStatus {
    fn get_order_batch(&self) -> Vec<Arc<RwLock<SolverOrder>>>;
    fn get_next_batch_order_id(&self) -> BatchOrderId;
    fn get_basket(&self, symbol: &Symbol) -> Option<Arc<Basket>>;
    fn get_prices(&self, price_type: PriceType, symbols: &[Symbol]) -> GetPricesResponse;
    fn get_liquidity(
        &self,
        side: Side,
        symbols: &HashMap<Symbol, Amount>,
    ) -> Result<HashMap<Symbol, Amount>>;
}

pub trait SolverStrategy {
    fn solve_engagements(
        &self,
        strategy_host: &dyn SolverStrategyHost,
    ) -> Result<Option<EngagedSolverOrders>>;
}

/// magic solver, needs to take index orders, and based on prices (from price
/// tracker) and available liquiduty (depth from order books), and active orders
/// (from order tracker) calculate best internal-portfolio rebalancing orders,
/// which will (partly) fill (some of the) ordered indexes.  Any position that
/// wasn't matched against ordered indexes shouldn't be kept for too long.
pub struct Solver {
    // solver strategy for calculating order batches
    strategy: Arc<dyn SolverStrategy>,
    batch_manager: Arc<BatchManager>,
    collateral_manager: Arc<CollateralManager>,
    basket_manager: Arc<RwLock<BasketManager>>,
    order_id_provider: Arc<RwLock<dyn OrderIdProvider>>,
    price_tracker: Arc<RwLock<PriceTracker>>,
    order_book_manager: Arc<RwLock<dyn OrderBookManager + Send + Sync>>,
    // dependencies
    chain_connector: Arc<RwLock<dyn ChainConnector + Send + Sync>>,
    index_order_manager: Arc<RwLock<IndexOrderManager>>,
    quote_request_manager: Arc<RwLock<dyn QuoteRequestManager + Send + Sync>>,
    inventory_manager: Arc<RwLock<InventoryManager>>,
    // orders
    client_orders: RwLock<HashMap<(Address, ClientOrderId), Arc<RwLock<SolverOrder>>>>,
    ready_orders: Mutex<VecDeque<Arc<RwLock<SolverOrder>>>>,
    // parameters
    max_batch_size: usize,
    zero_threshold: Amount,
}
impl Solver {
    pub fn new(
        strategy: Arc<dyn SolverStrategy>,
        order_id_provider: Arc<RwLock<dyn OrderIdProvider>>,
        basket_manager: Arc<RwLock<BasketManager>>,
        price_tracker: Arc<RwLock<PriceTracker>>,
        order_book_manager: Arc<RwLock<dyn OrderBookManager + Send + Sync>>,
        chain_connector: Arc<RwLock<dyn ChainConnector + Send + Sync>>,
        index_order_manager: Arc<RwLock<IndexOrderManager>>,
        quote_request_manager: Arc<RwLock<dyn QuoteRequestManager + Send + Sync>>,
        inventory_manager: Arc<RwLock<InventoryManager>>,
        max_batch_size: usize,
        zero_threshold: Amount,
        fill_threshold: Amount,
        mint_threshold: Amount,
        fund_wait_period: TimeDelta,
        mint_wait_period: TimeDelta,
    ) -> Self {
        Self {
            strategy,
            batch_manager: Arc::new(BatchManager::new(
                max_batch_size,
                zero_threshold,
                fill_threshold,
                mint_threshold,
                mint_wait_period,
            )),
            collateral_manager: Arc::new(CollateralManager::new(fund_wait_period)),
            order_id_provider,
            basket_manager,
            price_tracker,
            order_book_manager,
            // dependencies
            chain_connector,
            index_order_manager,
            quote_request_manager,
            inventory_manager,
            // orders
            client_orders: RwLock::new(HashMap::new()),
            ready_orders: Mutex::new(VecDeque::new()),
            // parameters
            max_batch_size,
            zero_threshold,
        }
    }

    fn engage_more_orders(&self) -> Result<()> {
        if let Some(engaged_orders) = self.strategy.solve_engagements(self)? {
            let send_engage = engaged_orders
                .engaged_orders
                .iter()
                .map_while(|order| {
                    let order = order.write();
                    let carried_collateral = order.index_order.read().collateral_carried;
                    if order.engaged_collateral < self.zero_threshold {
                        None
                    } else {
                        Some(EngageOrderRequest {
                            address: order.address,
                            client_order_id: order.client_order_id.clone(),
                            symbol: order.symbol.clone(),
                            // We already have engaged collateral that was
                            // carried over from previous batch so we only need
                            // to ask Index Order Manager to engage the
                            // difference.
                            collateral_amount: safe!(
                                order.engaged_collateral - carried_collateral
                            )?,
                        })
                    }
                })
                .collect_vec();

            if send_engage.len() != engaged_orders.engaged_orders.len() {
                return Err(eyre!("Zero Quantity"));
            }

            let batch_order_id = self.batch_manager.handle_new_engagement(engaged_orders)?;
            self.index_order_manager
                .write()
                .engage_orders(batch_order_id, send_engage)?;
        }
        Ok(())
    }

    fn mint_indexes(&self, timestamp: DateTime<Utc>) -> Result<()> {
        let ready_mints = self.batch_manager.get_mintable_batch(timestamp);

        for mintable_order in ready_mints {
            self.mint_index_order(&mut mintable_order.write())?;
        }

        Ok(())
    }

    fn mint_index_order(&self, index_order: &mut SolverOrder) -> Result<()> {
        let total_cost = index_order
            .lots
            .iter()
            .try_fold(Amount::ZERO, |cost, lot| {
                let lot_value = safe! {lot.price * lot.quantity}?;
                let cost = safe! {lot_value + lot.fee + cost}?;
                Some(cost)
            })
            .ok_or_eyre("Math Problem")?;

        let funds = self
            .collateral_manager
            .get_funds(&index_order.address)
            .ok_or_eyre("Missing funds")?;

        let mut funds_upread = funds.upgradable_read();

        if funds_upread.balance < total_cost {
            Err(eyre!("Not enough funds"))?;
        }

        let new_balance = safe!(funds_upread.balance - total_cost).ok_or_eyre("Math Problem")?;
        funds_upread.with_upgraded(|funds_write| funds_write.balance = new_balance);

        self.chain_connector.write().mint_index(
            index_order.symbol.clone(),
            index_order.filled_quantity,
            index_order.address,
            total_cost,
            index_order.timestamp,
        );

        self.set_order_status(index_order, SolverOrderStatus::Minted);

        Ok(())
    }

    /// Core thinking function
    pub fn solve(&self, timestamp: DateTime<Utc>) {
        println!("\nSolve...");

        //
        // check if there is some collateral we could use
        //
        if let Err(err) = self.collateral_manager.process_credits(self, timestamp) {
            eprintln!("Error while processing credits: {:?}", err);
        }

        //
        // check if there is some index orders we could mint
        //
        if let Err(err) = self.mint_indexes(timestamp) {
            eprintln!("Error while processing mints: {:?}", err);
        }

        //
        // NOTE: We should only engage new orders, and currently not much engaged
        // otherwise currently engaged orders may consume the liquidity.
        //
        // TODO: We may also track open liquidity promised to open orders
        //

        if let Err(err) = self.engage_more_orders() {
            eprintln!("Error while engaging more orders: {:?}", err);
        }

        //
        // NOTE: Index Order Manager will fire EngageIndexOrder event(s) and
        // we should move orders to new queue then, and here we could draw from
        // that new queue.
        //
        // So essentially:
        //  ( NewIndexOrder event ) => ready_queue =( find liquidity & engage )=> pending_queue
        //  ( EngageIndexOrder event ) => pending_queue =( move )=> engaged_queue
        //  ( solve ) => engaged_queue =( send order batch )=>  live_order_map
        //  ( inventory event ) => live_order_map =( match fill )=>

        // Compute symbols and threshold
        // ...

        // receive list of open lots from Inventory Manager
        //let _positions = self
        //    .inventory_manager
        //    .read()
        //    .get_positions(&engaged_orders.symbols);

        println!("Compute...");
        // Compute: Allocate open lots to Index Orders
        // ...
        // TBD: Should Solver or Inventory Manager be allocating lots to index orders?

        // Send back to Index Order Manager fills if any
        //self.index_order_manager
        //    .write()
        //    .fill_order_request(ClientOrderId::default(), Amount::default());

        // Compute: Remaining quantity
        // ...

        // receive current prices from Price Tracker

        // Compute: Orders to send to update inventory
        // ...

        // Send order requests to Inventory Manager
        // ...throttle these: send one or few smaller ones

        // TODO: Should throttling be done here in Solver or in Inventory Manager

        println!("\nSend Order Batches...");
        if let Err(err) = self.batch_manager.send_more_batches(self, timestamp) {
            eprintln!("Error while sending more batches: {:?}", err);
        }
    }

    /// Quoting function (fast)
    pub fn quote(&self, _quote_request: ()) {
        // Compute symbols and threshold
        // ...

        let symbols = [];

        // receive current prices from Price Tracker
        let prices = self
            .price_tracker
            .read()
            .get_prices(PriceType::VolumeWeighted, &symbols);

        // receive available liquidity from Order Book Manager
        let _liquidity = self
            .order_book_manager
            .read()
            .get_liquidity(Side::Sell, &prices.prices);

        // Compute: Quote with cost
        // ...

        // send back quote
        self.quote_request_manager.write().respond_quote(());
    }

    pub fn handle_chain_event(&self, notification: ChainNotification) -> Result<()> {
        match notification {
            ChainNotification::CuratorWeightsSet(symbol, basket_definition) => {
                println!("Solver: Handle Chain Event CuratorWeigthsSet {}", symbol);
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
                Ok(())
            }
            ChainNotification::Deposit {
                address,
                payment_id,
                amount,
                timestamp,
            } => self
                .collateral_manager
                .handle_deposit(address, payment_id, amount, timestamp),
            ChainNotification::WithdrawalRequest {
                address,
                payment_id,
                amount,
                timestamp,
            } => self
                .collateral_manager
                .handle_withdrawal(address, payment_id, amount, timestamp),
        }
    }

    /// receive Index Order
    pub fn handle_index_order(&self, notification: IndexOrderEvent) -> Result<()> {
        match notification {
            IndexOrderEvent::NewIndexOrder {
                original_client_order_id,
                address,
                client_order_id,
                payment_id,
                symbol,
                side,
                collateral_amount,
                timestamp,
            } => {
                println!(
                    "\nSolver: Handle Index Order NewIndexOrder {} {} < {} from {}",
                    symbol, original_client_order_id, client_order_id, address
                );
                match self
                    .client_orders
                    .write()
                    .entry((address, original_client_order_id.clone()))
                {
                    Entry::Vacant(entry) => {
                        let solver_order = Arc::new(RwLock::new(SolverOrder {
                            original_client_order_id: original_client_order_id.clone(),
                            address,
                            client_order_id,
                            payment_id,
                            symbol,
                            side,
                            remaining_collateral: collateral_amount,
                            engaged_collateral: Amount::ZERO,
                            collateral_carried: Amount::ZERO,
                            collateral_spent: Amount::ZERO,
                            filled_quantity: Amount::ZERO,
                            timestamp,
                            status: SolverOrderStatus::Open,
                            lots: Vec::new(),
                        }));
                        entry.insert(solver_order.clone());
                        self.collateral_manager.handle_new_index_order(solver_order);
                        //self.ready_orders.lock().push_back(solver_order);
                        todo!("Give collateral manager means to push solver order into ready");
                        Ok(())
                    }
                    Entry::Occupied(_) => {
                        todo!();
                    }
                }
            }
            IndexOrderEvent::UpdateIndexOrder {
                original_client_order_id,
                address,
                client_order_id,
                collateral_removed: _,
                collateral_remaining: _,
                timestamp: _,
            } => {
                println!(
                    "\nSolver: Handle Index Order UpdateIndexOrder{} < {} from {}",
                    original_client_order_id, client_order_id, address
                );
                todo!();
            }
            IndexOrderEvent::EngageIndexOrder {
                batch_order_id,
                engaged_orders,
                timestamp,
            } => self.batch_manager.handle_engage_index_order(
                self,
                batch_order_id,
                engaged_orders,
                timestamp,
            ),
            IndexOrderEvent::CancelIndexOrder {
                original_client_order_id,
                address,
                client_order_id,
                timestamp: _,
            } => {
                println!(
                    "\nSolver: Handle Index Order CancelIndexOrder {} < {} from {}",
                    original_client_order_id, client_order_id, address
                );
                todo!();
            }
        }
    }

    // receive QR
    pub fn handle_quote_request(&self, _notification: QuoteRequestEvent) {
        println!("\nSolver: Handle Quote Request");
        //self.quote(());
    }

    /// Receive fill notifications
    pub fn handle_inventory_event(&self, notification: InventoryEvent) -> Result<()> {
        match notification {
            InventoryEvent::OpenLot {
                order_id,
                batch_order_id,
                lot_id,
                symbol,
                side,
                price,
                quantity,
                fee,
                original_batch_quantity: _,
                batch_quantity_remaining: _,
                timestamp,
            } => {
                println!(
                    "\nSolver: Handle Inventory Event OpenLot {:?} {:5} {:0.5} @ {:0.5} + fee {:0.5} ({:0.3}%)",
                    side,
                    symbol,
                    quantity,
                    price,
                    fee,
                    (|| safe!(safe!(fee * Amount::ONE_HUNDRED) / safe!(quantity * price)?))().unwrap_or_default()
                );
                self.batch_manager.handle_new_lot(
                    self,
                    order_id,
                    batch_order_id,
                    lot_id,
                    symbol,
                    side,
                    price,
                    quantity,
                    fee,
                    timestamp,
                )
            }
            InventoryEvent::CloseLot {
                original_order_id: _,
                original_batch_order_id: _,
                original_lot_id: _,
                closing_order_id,
                closing_batch_order_id,
                closing_lot_id,
                symbol,
                side,
                original_price: _,
                closing_price,
                closing_fee,
                quantity_closed,
                original_quantity,
                quantity_remaining,
                closing_batch_original_quantity: _,
                closing_batch_quantity_remaining: _,
                original_timestamp: _,
                closing_timestamp,
            } => {
                println!(
                    "\nSolver: Handle Inventory Event CloseLot {:?} {:5} {:0.5}@{:0.5}+{:0.5} ({:0.5}%)",
                    side,
                    symbol,
                    quantity_closed,
                    closing_price,
                    closing_fee,
                    (|| safe!(safe!(Amount::ONE_HUNDRED * safe!(original_quantity - quantity_remaining)?)?
                        / original_quantity))().unwrap_or_default()
                );
                self.batch_manager.handle_new_lot(
                    self,
                    closing_order_id,
                    closing_batch_order_id,
                    closing_lot_id,
                    symbol,
                    side,
                    closing_price,
                    quantity_closed,
                    closing_fee,
                    closing_timestamp,
                )
            }
        }
    }

    /// receive current prices from Price Tracker
    pub fn handle_price_event(&self, notification: PriceEvent) {
        match notification {
            PriceEvent::PriceChange { symbol } => {
                println!("Solver: Handle Price Event {:5}", symbol)
            }
        };
    }

    /// receive available liquidity from Order Book Manager
    pub fn handle_book_event(&self, notification: OrderBookEvent) {
        match notification {
            OrderBookEvent::BookUpdate { symbol } => {
                println!("Solver: Handle Book Event {:5}", symbol);
            }
            OrderBookEvent::UpdateError { symbol, error } => {
                println!("Solver: Handle Book Event {:5}, Error: {}", symbol, error);
            }
        }
    }

    /// receive basket notification
    pub fn handle_basket_event(&self, notification: BasketNotification) {
        // TODO: (move this) once solvign is done notify new weights were applied
        match notification {
            BasketNotification::BasketAdded(symbol, basket) => {
                println!("Solver: Handle Basket Notification BasketAdded {}", symbol);
                self.chain_connector
                    .write()
                    .solver_weights_set(symbol, basket)
            }
            BasketNotification::BasketUpdated(symbol, basket) => {
                println!(
                    "Solver: Handle Basket Notification BasketUpdated {}",
                    symbol
                );
                self.chain_connector
                    .write()
                    .solver_weights_set(symbol, basket)
            }
            BasketNotification::BasketRemoved(symbol) => {
                println!(
                    "Solver: Handle Basket Notification BasketRemoved {}",
                    symbol
                );
                todo!()
            }
        }
    }
}

impl SetSolverOrderStatus for Solver {
    fn set_order_status(&self, order: &mut SolverOrder, status: SolverOrderStatus) {
        println!(
            "Set Index Order Status: {} {:?}",
            order.client_order_id, status
        );
        order.status = status;
    }
}

impl SolverStrategyHost for Solver {
    fn get_order_batch(&self) -> Vec<Arc<RwLock<SolverOrder>>> {
        let mut new_orders = self.ready_orders.lock();
        let max_drain = new_orders.len().min(self.max_batch_size);
        new_orders.drain(..max_drain).collect_vec()
    }

    fn get_next_batch_order_id(&self) -> BatchOrderId {
        self.order_id_provider.write().next_batch_order_id()
    }

    fn get_basket(&self, symbol: &Symbol) -> Option<Arc<Basket>> {
        self.basket_manager.read().get_basket(symbol).cloned()
    }

    fn get_prices(&self, price_type: PriceType, symbols: &[Symbol]) -> GetPricesResponse {
        self.price_tracker.read().get_prices(price_type, symbols)
    }

    fn get_liquidity(
        &self,
        side: Side,
        symbols: &HashMap<Symbol, Amount>,
    ) -> Result<HashMap<Symbol, Amount>> {
        self.order_book_manager.read().get_liquidity(side, symbols)
    }
}

impl BatchManagerHost for Solver {
    fn get_next_order_id(&self) -> OrderId {
        self.order_id_provider.write().next_order_id()
    }

    fn handle_orders_ready(&self, ready_orders: VecDeque<Arc<RwLock<SolverOrder>>>) {
        self.ready_orders.lock().extend(ready_orders);
    }

    fn send_order_batch(&self, batch_order: Arc<BatchOrder>) -> Result<()> {
        self.inventory_manager.write().new_order_batch(batch_order)
    }

    fn fill_order_request(
        &self,
        address: &Address,
        client_order_id: &ClientOrderId,
        symbol: &Symbol,
        collateral_spent: Amount,
        fill_amount: Amount,
        timestamp: DateTime<Utc>,
    ) -> Result<()> {
        self.index_order_manager.write().fill_order_request(
            address,
            client_order_id,
            symbol,
            collateral_spent,
            fill_amount,
            timestamp,
        )
    }
}

impl CollateralManagerHost for Solver {

}

#[cfg(test)]
mod test {
    use std::{any::type_name, sync::Arc, time::Duration};

    use chrono::Utc;
    use crossbeam::{
        channel::{unbounded, Sender},
        select,
    };
    use rust_decimal::dec;

    use crate::{
        assert_decimal_approx_eq,
        blockchain::chain_connector::test_util::{
            MockChainConnector, MockChainInternalNotification,
        },
        core::{
            bits::{PricePointEntry, SingleOrder},
            functional::{
                IntoNotificationHandlerOnceBox, IntoObservableMany, IntoObservableSingle,
                NotificationHandlerOnce,
            },
            test_util::{
                get_mock_address_1, get_mock_asset_1_arc, get_mock_asset_2_arc,
                get_mock_asset_name_1, get_mock_asset_name_2, get_mock_index_name_1,
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
        server::server::{test_util::MockServer, ServerEvent, ServerResponse},
        solver::{
            index_quote_manager::test_util::MockQuoteRequestManager, position::LotId,
            solvers::simple_solver::SimpleSolver,
        },
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

    struct MockOrderIdProvider {
        order_ids: VecDeque<OrderId>,
        batch_order_ids: VecDeque<BatchOrderId>,
    }

    impl OrderIdProvider for MockOrderIdProvider {
        fn next_order_id(&mut self) -> OrderId {
            self.order_ids.pop_front().expect("No more Order Ids")
        }
        fn next_batch_order_id(&mut self) -> BatchOrderId {
            self.batch_order_ids
                .pop_front()
                .expect("No more BatchOrder Ids")
        }
    }

    /// Test that solver system is sane
    ///
    /// Step 1.
    ///     - Send prices for assets (top of the book and last trade)
    ///     - Send book updates for assets (top two levels)
    ///     - Emit CuratorWeightsSet event from ChainConnector mock
    ///         - Solver should respond with updating baskets
    ///         - BasketManager should confirm basket updates
    ///     - Emit NewOrder event from FIX server mock
    ///         - Solver should receive NewIndexOrder event
    /// Tick 1.
    ///     - Solver engages with new index orders:
    ///         - Fetching prices and liquidity
    ///         - Calculating contribution
    ///         - Engaging in orders with IndexOrderManager
    ///             - Solver should receive EngageIndexOrder event from IndexOrderManager
    ///         - Solver will not send order batches yet
    /// Tick 2.
    ///     - Solver will not engage with no orders (no new orders)
    ///     - Solver sends out new order batches:
    ///         - Orders from the batch will reach OrderConnector, and we fill those orders
    ///         - Solver should receive OpenLot event from InventoryManager
    ///
    /// TODO:
    ///   Solver should redistribute any suitable quantity from inventory
    ///   accorting to contribution, and notify IndexOrderManager about filled
    ///   index orders
    ///
    #[test]
    fn sbe_solver() {
        let max_batch_size = 4;
        let tolerance = dec!(0.0001);
        let fund_wait_period = TimeDelta::new(600, 0).unwrap();
        let mint_wait_period = TimeDelta::new(600, 0).unwrap();

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

        let order_id_provider = Arc::new(RwLock::new(MockOrderIdProvider {
            order_ids: VecDeque::from_iter(
                ["Order01", "Order02", "Order03", "Order04"]
                    .into_iter()
                    .map_into(),
            ),
            batch_order_ids: VecDeque::from_iter(["Batch01", "Batch02"].into_iter().map_into()),
        }));

        let solver_strategy = Arc::new(SimpleSolver::new(
            dec!(0.01),
            dec!(1.001),
            dec!(3000.0),
            dec!(2000.0),
        ));

        let solver = Arc::new(Solver::new(
            solver_strategy.clone(),
            order_id_provider.clone(),
            basket_manager.clone(),
            price_tracker.clone(),
            order_book_manager.clone(),
            chain_connector.clone(),
            index_order_manager.clone(),
            quote_request_manager.clone(),
            inventory_manager.clone(),
            max_batch_size,
            tolerance,
            dec!(0.9999),
            dec!(0.99),
            fund_wait_period,
            mint_wait_period,
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

        let order_tracker_2 = order_tracker.clone();

        let lot_ids = RwLock::new(VecDeque::<LotId>::from([
            "Lot01".into(),
            "Lot02".into(),
            "Lof03".into(),
            "Lot04".into(),
        ]));
        let order_connector_weak = Arc::downgrade(&order_connector);
        let (defer_1, deferred) = unbounded::<Box<dyn FnOnce() + Send + Sync>>();
        order_connector
            .write()
            .implementor
            .set_observer_fn(move |e: Arc<SingleOrder>| {
                let order_connector = order_connector_weak.upgrade().unwrap();
                let lot_id = lot_ids.write().pop_front().unwrap();
                let p1 = e.price
                    * match e.side {
                        Side::Buy => dec!(0.995),
                        Side::Sell => dec!(1.005),
                    };
                let p2 = e.price
                    * match e.side {
                        Side::Buy => dec!(0.998),
                        Side::Sell => dec!(1.002),
                    };
                let q1 = e.quantity * dec!(0.8);
                let q2 = e.quantity * dec!(0.2);
                let defer = defer_1.clone();
                // Note we defer first fill to make sure we don't get dead-lock
                defer_1
                    .send(Box::new(move || {
                        order_connector.write().notify_fill(
                            e.order_id.clone(),
                            lot_id.clone(),
                            e.symbol.clone(),
                            e.side,
                            p1,
                            q1,
                            dec!(0.001) * p1 * q1,
                            e.created_timestamp,
                        );
                        // We defer second fill, so that fills of different orders
                        // will be interleaved. We do that to test progressive fill-rate
                        // of the Index Order in our simulation.
                        defer
                            .send(Box::new(move || {
                                order_connector.write().notify_fill(
                                    e.order_id.clone(),
                                    lot_id.clone(),
                                    e.symbol.clone(),
                                    e.side,
                                    p2,
                                    q2,
                                    dec!(0.001) * p2 * q2,
                                    e.created_timestamp,
                                );
                            }))
                            .unwrap();
                    }))
                    .unwrap();
            });

        let (mock_chain_sender, mock_chain_receiver) = unbounded::<MockChainInternalNotification>();
        let (mock_fix_sender, mock_fix_receiver) = unbounded::<ServerResponse>();

        chain_connector
            .write()
            .implementor
            .set_observer_fn(move |response| {
                match &response {
                    MockChainInternalNotification::SolverWeightsSet(symbol, _) => {
                        println!("Solver Weights Set: {}", symbol);
                    }
                    MockChainInternalNotification::MintIndex {
                        symbol,
                        quantity,
                        receipient,
                        execution_price,
                        execution_time,
                    } => {
                        println!(
                            "Minted Index: {:5} Quantity: {:0.5} User: {} @{:0.5} {}",
                            symbol, quantity, receipient, execution_price, execution_time
                        );
                    }
                    MockChainInternalNotification::BurnIndex {
                        symbol,
                        quantity,
                        receipient,
                    } => {
                        todo!()
                    }
                    MockChainInternalNotification::Withdraw {
                        receipient,
                        amount,
                        execution_price,
                        execution_time,
                    } => {
                        todo!()
                    }
                };
                mock_chain_sender
                    .send(response)
                    .expect("Failed to send chain response");
                println!("Chain response sent.");
            });

        fix_server
            .write()
            .implementor
            .set_observer_fn(move |response| {
                match &response {
                    ServerResponse::NewIndexOrderAck {
                        address,
                        client_order_id,
                        timestamp,
                    } => {
                        println!(
                            "FIX Response: {} {} {}",
                            address, client_order_id, timestamp
                        );
                    }
                    ServerResponse::IndexOrderFill {
                        address,
                        client_order_id,
                        filled_quantity,
                        collateral_remaining,
                        collateral_spent,
                        timestamp,
                    } => {
                        println!(
                            "FIX Response: {} {} {:0.5} {:0.5} {:0.5} {}",
                            address,
                            client_order_id,
                            filled_quantity,
                            collateral_remaining,
                            collateral_spent,
                            timestamp
                        );
                    }
                };
                mock_fix_sender
                    .send(response)
                    .expect("Failed to send FIX response");
                println!("FIX response sent.");
            });

        let (solver_tick_sender, solver_tick_receiver) = unbounded::<DateTime<Utc>>();
        let solver_tick = |msg| solver_tick_sender.send(msg).unwrap();

        let flush_events = move || {
            // Simple dispatch loop
            loop {
                select! {
                    recv(quote_request_receiver) -> res => solver.handle_quote_request(res.unwrap()),
                    recv(price_receiver) -> res => solver.handle_price_event(res.unwrap()),
                    recv(book_receiver) -> res => solver.handle_book_event(res.unwrap()),
                    recv(basket_receiver) -> res => solver.handle_basket_event(res.unwrap()),

                    recv(chain_receiver) -> res => solver.handle_chain_event(res.unwrap())
                        .map_err(|e| eyre!("{:?}", e))
                        .expect("Failed to handle chain event"),

                    recv(inventory_receiver) -> res => solver.handle_inventory_event(res.unwrap())
                        .map_err(|e| eyre!("{:?}", e))
                        .expect("Failed to handle inventory event"),

                    recv(index_order_receiver) -> res => solver.handle_index_order(res.unwrap())
                        .map_err(|e| eyre!("{:?}", e))
                        .expect("Failed to handle inventory event"),

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
                    recv(deferred) -> res => (res.unwrap())(),
                    recv(solver_tick_receiver) -> res => {
                        solver.solve(res.unwrap())
                    },
                    default => { break; },
                }
            }
        };

        let mut timestamp = Utc::now();

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
            dec!(90.0),
            dec!(100.0),
            dec!(10.0),
            dec!(20.0),
        );

        market_data_connector.write().notify_top_of_book(
            get_mock_asset_name_2(),
            dec!(295.0),
            dec!(300.0),
            dec!(80.0),
            dec!(50.0),
        );

        // last trade
        market_data_connector
            .write()
            .notify_trade(get_mock_asset_name_1(), dec!(90.0), dec!(5.0));

        market_data_connector.write().notify_trade(
            get_mock_asset_name_2(),
            dec!(300.0),
            dec!(15.0),
        );

        // book depth
        market_data_connector.write().notify_full_order_book(
            get_mock_asset_name_1(),
            vec![
                PricePointEntry {
                    price: dec!(90.0),
                    quantity: dec!(10.0),
                },
                PricePointEntry {
                    price: dec!(80.0),
                    quantity: dec!(40.0),
                },
            ],
            vec![
                PricePointEntry {
                    price: dec!(100.0),
                    quantity: dec!(20.0),
                },
                PricePointEntry {
                    price: dec!(110.0),
                    quantity: dec!(30.0),
                },
            ],
        );

        market_data_connector.write().notify_full_order_book(
            get_mock_asset_name_2(),
            vec![
                PricePointEntry {
                    price: dec!(295.0),
                    quantity: dec!(80.0),
                },
                PricePointEntry {
                    price: dec!(290.0),
                    quantity: dec!(100.0),
                },
            ],
            vec![
                PricePointEntry {
                    price: dec!(300.0),
                    quantity: dec!(50.0),
                },
                PricePointEntry {
                    price: dec!(305.0),
                    quantity: dec!(150.0),
                },
            ],
        );

        // necessary to wait until all market data events are ingested
        flush_events();

        // define basket
        let basket_definition = BasketDefinition::try_new(vec![
            AssetWeight::new(get_mock_asset_1_arc(), dec!(0.8)),
            AssetWeight::new(get_mock_asset_2_arc(), dec!(0.2)),
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

        assert!(matches!(
            solver_weithgs_set,
            MockChainInternalNotification::SolverWeightsSet(_, _)
        ));

        print!("Chain response received.");

        let collateral_amount = dec!(1005.0) * dec!(3.5) * (Amount::ONE + dec!(0.001));

        chain_connector.write().notify_deposit(
            get_mock_address_1(),
            "Pay001".into(),
            collateral_amount,
            timestamp,
        );

        flush_events();

        println!("We sent deposit");
        solver_tick(timestamp);

        flush_events();

        fix_server
            .write()
            .notify_server_event(Arc::new(ServerEvent::NewIndexOrder {
                address: get_mock_address_1(),
                client_order_id: "Order01".into(),
                payment_id: "Pay001".into(),
                symbol: get_mock_index_name_1(),
                side: Side::Buy,
                collateral_amount,
                timestamp,
            }));

        flush_events();

        println!("We sent NewOrderSingle FIX message");
        solver_tick(timestamp);

        flush_events();

        println!("IndexOrderManager responded to EngageOrders");
        solver_tick(timestamp);

        flush_events();

        let fix_response = mock_fix_receiver
            .recv_timeout(Duration::from_secs(1))
            .expect("Failed to receive ServerResponse");

        assert!(matches!(
            fix_response,
            ServerResponse::NewIndexOrderAck {
                address: _,
                client_order_id: _,
                timestamp: _
            }
        ));

        let order1 = order_tracker_2.read().get_order(&"Order01".into());
        let order2 = order_tracker_2.read().get_order(&"Order02".into());

        assert!(matches!(order1, Some(_)));
        assert!(matches!(order2, Some(_)));
        let order1 = order1.unwrap();
        let order2 = order2.unwrap();

        assert_eq!(order1.symbol, get_mock_asset_name_1());
        assert_eq!(order2.symbol, get_mock_asset_name_2());
        assert_eq!(order1.side, Side::Buy);
        assert_eq!(order2.side, Side::Buy);

        assert_decimal_approx_eq!(order1.price, dec!(101.00), tolerance);
        assert_decimal_approx_eq!(order1.quantity, dec!(15.9158), tolerance);
        assert_decimal_approx_eq!(order2.price, dec!(303.00), tolerance);
        assert_decimal_approx_eq!(order2.quantity, dec!(1.2954), tolerance);

        flush_events();

        for _ in 0..2 {
            let fix_response = mock_fix_receiver
                .recv_timeout(Duration::from_secs(1))
                .expect("Failed to receive ServerResponse");

            assert!(matches!(
                fix_response,
                ServerResponse::IndexOrderFill {
                    address: _,
                    client_order_id: _,
                    filled_quantity: _,
                    collateral_remaining: _,
                    collateral_spent: _,
                    timestamp: _
                }
            ));

            println!("FIX response received.");
        }

        println!("Solver re-inserts IndexOrder from filled batch");
        solver_tick(timestamp);

        flush_events();

        println!("Solver sends next batch");
        solver_tick(timestamp);

        flush_events();

        for _ in 0..2 {
            let fix_response = mock_fix_receiver
                .recv_timeout(Duration::from_secs(1))
                .expect("Failed to receive ServerResponse");

            assert!(matches!(
                fix_response,
                ServerResponse::IndexOrderFill {
                    address: _,
                    client_order_id: _,
                    filled_quantity: _,
                    collateral_remaining: _,
                    collateral_spent: _,
                    timestamp: _
                }
            ));

            println!("FIX response received.");
        }

        println!("We moved clock 10 minutes forward");
        timestamp += fund_wait_period;
        solver_tick(timestamp);

        flush_events();

        // wait for solver to solve...
        let mint_index = mock_chain_receiver
            .recv_timeout(Duration::from_secs(1))
            .expect("Failed to receive MintIndex");

        assert!(matches!(
            mint_index,
            MockChainInternalNotification::MintIndex {
                symbol: _,
                quantity: _,
                receipient: _,
                execution_price: _,
                execution_time: _
            }
        ));

        println!("Chain response received.");

        println!("Scenario completed.")
    }
}
