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
    collateral::collateral_manager::{
        CollateralEvent, CollateralManager, CollateralManagerHost, PreAuthStatus,
    },
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
    // Chain ID
    pub chain_id: u32,

    // ID of the original NewOrder request
    pub original_client_order_id: ClientOrderId,

    /// On-chain address of the User
    pub address: Address,

    // ID of the NewOrder request
    pub client_order_id: ClientOrderId,

    /// An ID of the on-chain payment
    pub payment_id: Option<PaymentId>,

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

pub struct SolveEngagementsResult {
    pub engaged_orders: EngagedSolverOrders,
    pub failed_orders: Vec<Arc<RwLock<SolverOrder>>>,
}

pub struct CollateralManagement {
    pub chain_id: u32,
    pub address: Address,
    pub client_order_id: ClientOrderId,
    pub side: Side,
    pub collateral_amount: Amount,
    pub asset_requirements: HashMap<Symbol, Amount>,
}

pub trait OrderIdProvider {
    fn next_order_id(&mut self) -> OrderId;
    fn next_batch_order_id(&mut self) -> BatchOrderId;
    fn next_payment_id(&mut self) -> PaymentId;
}

pub trait SetSolverOrderStatus {
    fn set_order_status(&self, order: &mut SolverOrder, status: SolverOrderStatus);
}

pub trait SolverStrategyHost: SetSolverOrderStatus {
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
    fn query_collateral_management(
        &self,
        strategy_host: &dyn SolverStrategyHost,
        order: Arc<RwLock<SolverOrder>>,
    ) -> Result<CollateralManagement>;

    fn solve_engagements(
        &self,
        strategy_host: &dyn SolverStrategyHost,
        order_batch: Vec<Arc<RwLock<SolverOrder>>>,
    ) -> Result<SolveEngagementsResult>;
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
    basket_manager: Arc<RwLock<BasketManager>>,
    order_id_provider: Arc<RwLock<dyn OrderIdProvider>>,
    price_tracker: Arc<RwLock<PriceTracker>>,
    order_book_manager: Arc<RwLock<dyn OrderBookManager + Send + Sync>>,
    // dependencies
    chain_connector: Arc<RwLock<dyn ChainConnector + Send + Sync>>,
    collateral_manager: Arc<RwLock<CollateralManager>>,
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
        collateral_manager: Arc<RwLock<CollateralManager>>,
        index_order_manager: Arc<RwLock<IndexOrderManager>>,
        quote_request_manager: Arc<RwLock<dyn QuoteRequestManager + Send + Sync>>,
        inventory_manager: Arc<RwLock<InventoryManager>>,
        max_batch_size: usize,
        zero_threshold: Amount,
        fill_threshold: Amount,
        mint_threshold: Amount,
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
            order_id_provider,
            basket_manager,
            price_tracker,
            order_book_manager,
            // dependencies
            chain_connector,
            collateral_manager,
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

    fn get_order_batch(&self) -> Vec<Arc<RwLock<SolverOrder>>> {
        let mut new_orders = self.ready_orders.lock();
        let max_drain = new_orders.len().min(self.max_batch_size);
        new_orders.drain(..max_drain).collect_vec()
    }

    fn handle_failed_orders(&self, failed_orders: Vec<Arc<RwLock<SolverOrder>>>) {
        for failed_order in failed_orders {
            let failed_status = failed_order.read().status;
            match failed_status {
                SolverOrderStatus::MissingPrices => {
                    // TODO: We could have nother queue with time delay
                    self.ready_orders.lock().push_back(failed_order)
                }
                _ => todo!("Send NAK"),
            }
        }
    }

    fn engage_more_orders(&self) -> Result<()> {
        let order_batch = self.get_order_batch();

        if order_batch.is_empty() {
            return Ok(());
        }

        let solve_engagements_result = self.strategy.solve_engagements(self, order_batch)?;

        self.handle_failed_orders(solve_engagements_result.failed_orders);

        if !solve_engagements_result
            .engaged_orders
            .engaged_orders
            .is_empty()
        {
            let engaged_orders = solve_engagements_result.engaged_orders;
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
                todo!("We got some error! Send NAKs")
            }

            let batch_order_id = self.batch_manager.handle_new_engagement(engaged_orders)?;
            self.index_order_manager
                .write()
                .engage_orders(batch_order_id, send_engage)?;
        }
        Ok(())
    }

    fn mint_index_order(
        &self,
        index_order: &mut SolverOrder,
        timestamp: DateTime<Utc>,
    ) -> Result<()> {
        let total_cost = index_order
            .lots
            .iter()
            .try_fold(Amount::ZERO, |cost, lot| {
                let lot_value = safe! {lot.price * lot.quantity}?;
                let cost = safe! {lot_value + lot.fee + cost}?;
                Some(cost)
            })
            .ok_or_eyre("Math Problem")?;

        let payment_id = index_order
            .payment_id
            .clone()
            .ok_or_eyre("Missing payment ID")?;

        self.collateral_manager.write().confirm_payment(
            index_order.chain_id,
            &index_order.address,
            &payment_id,
            timestamp,
            index_order.side,
            total_cost,
        )?;

        self.chain_connector.write().mint_index(
            index_order.chain_id,
            index_order.symbol.clone(),
            index_order.filled_quantity,
            index_order.address,
            total_cost,
            index_order.timestamp,
        );

        self.set_order_status(index_order, SolverOrderStatus::Minted);

        Ok(())
    }

    fn mint_indexes(&self, timestamp: DateTime<Utc>) -> Result<()> {
        let ready_mints = self.batch_manager.get_mintable_batch(timestamp);

        for mintable_order in ready_mints {
            self.mint_index_order(&mut mintable_order.write(), timestamp)?;
        }

        Ok(())
    }

    /// Core thinking function
    pub fn solve(&self, timestamp: DateTime<Utc>) {
        println!("\n(solver) Begin solve");

        //
        // check if there is some collateral we could use
        //
        println!("(solver) * Process credits");
        if let Err(err) = self
            .collateral_manager
            .write()
            .process_credits(self, timestamp)
        {
            eprintln!("(solver) Error while processing credits: {:?}", err);
        }

        //
        // check if there is some index orders we could mint
        //
        println!("(solver) * Mint indexes");
        if let Err(err) = self.mint_indexes(timestamp) {
            eprintln!("(solver) Error while processing mints: {:?}", err);
        }

        //
        // NOTE: We should only engage new orders, and currently not much engaged
        // otherwise currently engaged orders may consume the liquidity.
        //
        // TODO: We may also track open liquidity promised to open orders
        //
        println!("(solver) * Engage more orders");
        if let Err(err) = self.engage_more_orders() {
            eprintln!("Error while engaging more orders: {:?}", err);
        }

        println!("(solver) * Send more batches");
        if let Err(err) = self.batch_manager.send_more_batches(self, timestamp) {
            eprintln!("(solver) Error while sending more batches: {:?}", err);
        }

        println!("(solver) End solve\n");
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
                println!("(solver) Handle Chain Event CuratorWeigthsSet {}", symbol);
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
                        "(solver) No prices available for some symbols: {:?}",
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
                    println!("(solver) Error while setting curator weights: {err}");
                }
                Ok(())
            }
            ChainNotification::Deposit {
                chain_id,
                address,
                amount,
                timestamp,
            } => self
                .collateral_manager
                .write()
                .handle_deposit(self, chain_id, address, amount, timestamp),
            ChainNotification::WithdrawalRequest {
                chain_id,
                address,
                amount,
                timestamp,
            } => self
                .collateral_manager
                .write()
                .handle_withdrawal(self, chain_id, address, amount, timestamp),
        }
    }

    pub fn handle_collateral_event(&self, notificaion: CollateralEvent) -> Result<()> {
        match notificaion {
            CollateralEvent::CollateralReady {
                chain_id,
                address,
                client_order_id,
                collateral_amount,
                fee,
                timestamp,
            } => {
                println!(
                    "(solver) CollateralReady for {} {} {:0.5} {:0.5}",
                    chain_id, address, collateral_amount, fee
                );
                if let Some(order) = self
                    .client_orders
                    .read()
                    .get(&(address, client_order_id.clone()))
                {
                    // TODO: Figure out: should collateral manager have already paid for the order?
                    // or CollateralEvent is only to tell us that collateral reached sub-accounts?
                    // NOTE: Paying for order, is just telling collateral manager to block certain
                    // amount of balance, so that any next order from that user won't double-spend.
                    // We assign payment ID so that  we can identify association between order and
                    // allocated collateral.
                    let side = order.read().side;
                    self.collateral_manager.write().preauth_payment(
                        self,
                        chain_id,
                        address,
                        client_order_id,
                        timestamp,
                        side,
                        collateral_amount,
                    )?;
                }
            }
            CollateralEvent::PreAuthResponse {
                chain_id: _,
                address,
                client_order_id,
                amount_payable: _,
                status,
            } => {
                if let Some(order) = self.client_orders.read().get(&(address, client_order_id)) {
                    match status {
                        // If we're implementing message based protocol, we should make PaymentApproved
                        // a message that we will receive from collateral manager.
                        PreAuthStatus::Approved { payment_id } => {
                            println!("(solver) PaymentApproved: {}", payment_id);
                            let mut order_write = order.write();
                            order_write
                                .payment_id
                                .replace(payment_id)
                                .is_none()
                                .then_some(())
                                .ok_or_eyre("Payment ID already set")?;
                            self.set_order_status(&mut order_write, SolverOrderStatus::Ready);
                            self.ready_orders.lock().push_back(order.clone());
                        }
                        PreAuthStatus::NotEnoughFunds => {
                            eprintln!("(solver) Not enough funds")
                        }
                    }
                }
            }
        }
        Ok(())
    }

    /// receive Index Order
    pub fn handle_index_order(&self, notification: IndexOrderEvent) -> Result<()> {
        match notification {
            IndexOrderEvent::NewIndexOrder {
                chain_id,
                original_client_order_id,
                address,
                client_order_id,
                symbol,
                side,
                collateral_amount,
                timestamp,
            } => {
                println!(
                    "\n(solver) Handle Index Order NewIndexOrder {} {} < {} from {}",
                    symbol, original_client_order_id, client_order_id, address
                );
                match self
                    .client_orders
                    .write()
                    .entry((address, original_client_order_id.clone()))
                {
                    Entry::Vacant(entry) => {
                        let solver_order = Arc::new(RwLock::new(SolverOrder {
                            chain_id,
                            original_client_order_id: original_client_order_id.clone(),
                            address,
                            client_order_id,
                            payment_id: None,
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
                        let collateral_management = self
                            .strategy
                            .query_collateral_management(self, solver_order)?;
                        self.collateral_manager
                            .write()
                            .manage_collateral(collateral_management);
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
                    "\n(solver) Handle Index Order UpdateIndexOrder{} < {} from {}",
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
                    "\n(solver) Handle Index Order CancelIndexOrder {} < {} from {}",
                    original_client_order_id, client_order_id, address
                );
                todo!();
            }
        }
    }

    // receive QR
    pub fn handle_quote_request(&self, _notification: QuoteRequestEvent) {
        println!("\n(solver) Handle Quote Request");
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
                    "\n(solver) Handle Inventory Event OpenLot {:?} {:5} {:0.5} @ {:0.5} + fee {:0.5} ({:0.3}%)",
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
                    "\n(solver) Handle Inventory Event CloseLot {:?} {:5} {:0.5}@{:0.5}+{:0.5} ({:0.5}%)",
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
                println!("(solver) Handle Price Event {:5}", symbol)
            }
        };
    }

    /// receive available liquidity from Order Book Manager
    pub fn handle_book_event(&self, notification: OrderBookEvent) {
        match notification {
            OrderBookEvent::BookUpdate { symbol } => {
                println!("(solver) Handle Book Event {:5}", symbol);
            }
            OrderBookEvent::UpdateError { symbol, error } => {
                println!("(solver) Handle Book Event {:5}, Error: {}", symbol, error);
            }
        }
    }

    /// receive basket notification
    pub fn handle_basket_event(&self, notification: BasketNotification) {
        // TODO: (move this) once solvign is done notify new weights were applied
        match notification {
            BasketNotification::BasketAdded(symbol, basket) => {
                println!("(solver) Handle Basket Notification BasketAdded {}", symbol);
                self.chain_connector
                    .write()
                    .solver_weights_set(symbol, basket)
            }
            BasketNotification::BasketUpdated(symbol, basket) => {
                println!(
                    "(solver) Handle Basket Notification BasketUpdated {}",
                    symbol
                );
                self.chain_connector
                    .write()
                    .solver_weights_set(symbol, basket)
            }
            BasketNotification::BasketRemoved(symbol) => {
                println!(
                    "(solver) Handle Basket Notification BasketRemoved {}",
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
            "(solver) Set Index Order Status: {} {:?}",
            order.client_order_id, status
        );
        order.status = status;
    }
}

impl SolverStrategyHost for Solver {
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
    fn get_next_payment_id(&self) -> PaymentId {
        self.order_id_provider.write().next_payment_id()
    }
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
        collateral::collateral_router::{
            test_util::{
                MockCollateralBridge, MockCollateralBridgeInternalEvent, MockCollateralDesignation,
            },
            CollateralDesignation, CollateralRouter, CollateralRouterEvent,
            CollateralTransferEvent,
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
        payment_ids: VecDeque<PaymentId>,
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

        fn next_payment_id(&mut self) -> PaymentId {
            self.payment_ids
                .pop_front()
                .expect("No more PaymentIds Ids")
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
        let collateral_amount = dec!(1005.0) * dec!(3.5) * (Amount::ONE + dec!(0.001));
        let chain_id = 1;
        let max_batch_size = 4;
        let tolerance = dec!(0.0001);
        let fund_wait_period = TimeDelta::new(600, 0).unwrap();
        let mint_wait_period = TimeDelta::new(600, 0).unwrap();

        let (chain_sender, chain_receiver) = unbounded::<ChainNotification>();
        let (collateral_sender, collateral_receiver) = unbounded::<CollateralEvent>();
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
        let (collateral_router_sender, collateral_router_receiver) =
            unbounded::<CollateralRouterEvent>();
        let (collateral_transfer_sender, collateral_transfer_receiver) =
            unbounded::<CollateralTransferEvent>();

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

        let collateral_designation_1 = Arc::new(RwLock::new(MockCollateralDesignation {
            type_: "T1".into(),
            name: "D1".into(),
            collateral_symbol: "C1".into(),
            full_name: "T1:D1:C1".into(),
            balance: dec!(0.0),
        }));

        let collateral_designation_2 = Arc::new(RwLock::new(MockCollateralDesignation {
            type_: "T2".into(),
            name: "D2".into(),
            collateral_symbol: "C2".into(),
            full_name: "T2:D2:C2".into(),
            balance: dec!(0.0),
        }));

        let collateral_designation_3 = Arc::new(RwLock::new(MockCollateralDesignation {
            type_: "T3".into(),
            name: "D3".into(),
            collateral_symbol: "C3".into(),
            full_name: "T3:D3:C3".into(),
            balance: dec!(0.0),
        }));

        let collateral_bridge_1 = Arc::new(RwLock::new(MockCollateralBridge::new(
            collateral_designation_1.clone(),
            collateral_designation_2.clone(),
        )));

        let collateral_bridge_2 = Arc::new(RwLock::new(MockCollateralBridge::new(
            collateral_designation_2.clone(),
            collateral_designation_3.clone(),
        )));

        let collateral_router = Arc::new(RwLock::new(CollateralRouter::new()));

        collateral_router
            .write()
            .add_bridge(collateral_bridge_1.clone())
            .expect("Failed to add bridge");

        collateral_router
            .write()
            .add_bridge(collateral_bridge_2.clone())
            .expect("Failed to add bridge");

        collateral_router
            .write()
            .add_chain_source(
                chain_id,
                collateral_designation_1.read().get_full_name().clone(),
            )
            .expect("Failed to add chain source");

        collateral_router
            .write()
            .set_default_destination(collateral_designation_3.read().get_full_name().clone())
            .expect("Failed to set default destination");

        collateral_router
            .write()
            .add_route(&[
                collateral_designation_1.read().get_full_name().clone(),
                collateral_designation_2.read().get_full_name().clone(),
                collateral_designation_3.read().get_full_name().clone(),
            ])
            .expect("Failed to add route");

        let collateral_manager = Arc::new(RwLock::new(CollateralManager::new(
            collateral_router.clone(),
            tolerance,
        )));

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
            payment_ids: VecDeque::from_iter(["Payment01", "Payment02"].into_iter().map_into()),
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
            collateral_manager.clone(),
            index_order_manager.clone(),
            quote_request_manager.clone(),
            inventory_manager.clone(),
            max_batch_size,
            tolerance,
            dec!(0.9999),
            dec!(0.99),
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

        collateral_manager
            .write()
            .get_single_observer_mut()
            .set_observer_from(collateral_sender);

        collateral_bridge_1
            .write()
            .get_single_observer_mut()
            .set_observer_from(collateral_router_sender.clone());

        collateral_bridge_2
            .write()
            .get_single_observer_mut()
            .set_observer_from(collateral_router_sender);

        collateral_router
            .write()
            .get_single_observer_mut()
            .set_observer_from(collateral_transfer_sender);

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
        let defer_2 = defer_1.clone();
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
                println!(
                    "(mock) SingleOrder {} {} {:0.5} @ {:0.5} {:0.5} @ {:0.5}",
                    e.symbol, lot_id, q1, p1, q2, p2
                );
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
        let (mock_bridge_sender, mock_bridge_receiver) =
            unbounded::<MockCollateralBridgeInternalEvent>();
        let (mock_fix_sender, mock_fix_receiver) = unbounded::<ServerResponse>();

        chain_connector
            .write()
            .implementor
            .set_observer_fn(move |response| {
                match &response {
                    MockChainInternalNotification::SolverWeightsSet(symbol, _) => {
                        println!("(mock) SolverWeightsSet: {}", symbol);
                    }
                    MockChainInternalNotification::MintIndex {
                        chain_id,
                        symbol,
                        quantity,
                        receipient,
                        execution_price,
                        execution_time,
                    } => {
                        println!(
                            "(mock) MintedIndex: {} {:5} Quantity: {:0.5} User: {} @{:0.5} {}",
                            chain_id, symbol, quantity, receipient, execution_price, execution_time
                        );
                    }
                    MockChainInternalNotification::BurnIndex {
                        chain_id,
                        symbol,
                        quantity,
                        receipient,
                    } => {
                        todo!()
                    }
                    MockChainInternalNotification::Withdraw {
                        chain_id,
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
                println!("(mock) Chain response sent");
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
                            "(mock) FIX Response: {} {} {}",
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
                            "(mock) FIX Response: {} {} {:0.5} {:0.5} {:0.5} {}",
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
                println!("(mock) FIX response sent");
            });

        let impl_collateral_bridge =
            (move |collateral_bridge: &Arc<RwLock<MockCollateralBridge>>,
                   mock_bridge_sender: Sender<MockCollateralBridgeInternalEvent>,
                   defer_2: Sender<Box<dyn FnOnce() + Send + Sync>>| {
                let collateral_bridge_weak = Arc::downgrade(collateral_bridge);
                collateral_bridge
                    .write()
                    .implementor
                    .set_observer_fn(move |event| {
                        let collateral_bridge = collateral_bridge_weak.upgrade().unwrap();
                        match &event {
                            MockCollateralBridgeInternalEvent::TransferFunds {
                                chain_id,
                                address,
                                client_order_id,
                                route_from,
                                route_to,
                                amount,
                            } => {
                                println!(
                                    "(mock) TransferFunds: from {} {} {} for {:0.5}",
                                    chain_id, address, client_order_id, amount
                                );
                                let chain_id = *chain_id;
                                let address = *address;
                                let client_order_id = client_order_id.clone();
                                let route_from = route_from.clone();
                                let route_to = route_to.clone();
                                let amount = *amount;
                                let fee = amount * dec!(0.01);
                                let timestamp = Utc::now();
                                defer_2
                                    .send(Box::new(move || {
                                        collateral_bridge.write().notify_collateral_router_event(
                                            chain_id,
                                            address,
                                            client_order_id,
                                            timestamp,
                                            route_from,
                                            route_to,
                                            amount - fee,
                                            fee,
                                        );
                                    }))
                                    .expect("Failed to send");
                            }
                        }
                        mock_bridge_sender
                            .send(event)
                            .expect("Failed to send bridge event");
                        println!("(mock) Bridge event sent");
                    });
            });

        impl_collateral_bridge(
            &collateral_bridge_1,
            mock_bridge_sender.clone(),
            defer_2.clone(),
        );
        impl_collateral_bridge(
            &collateral_bridge_2,
            mock_bridge_sender.clone(),
            defer_2.clone(),
        );

        let (solver_tick_sender, solver_tick_receiver) = unbounded::<DateTime<Utc>>();
        let solver_tick = |msg| solver_tick_sender.send(msg).unwrap();

        let flush_events = move || {
            // Simple dispatch loop
            println!("\n>>> Begin events");
            loop {
                select! {
                    recv(quote_request_receiver) -> res => solver.handle_quote_request(res.unwrap()),
                    recv(price_receiver) -> res => solver.handle_price_event(res.unwrap()),
                    recv(book_receiver) -> res => solver.handle_book_event(res.unwrap()),
                    recv(basket_receiver) -> res => solver.handle_basket_event(res.unwrap()),

                    recv(chain_receiver) -> res => solver.handle_chain_event(res.unwrap())
                        .map_err(|e| eyre!("{:?}", e))
                        .expect("Failed to handle chain event"),

                    recv(collateral_receiver) -> res => solver.handle_collateral_event(res.unwrap())
                        .map_err(|e| eyre!("{:?}", e))
                        .expect("Failed to handle collateral event"),

                    recv(collateral_transfer_receiver) -> res => solver.collateral_manager.write()
                        .handle_collateral_transfer_event(res.unwrap())
                        .map_err(|e| eyre!("{:?}", e))
                        .expect("Failed to handle bridge event"),

                    recv(collateral_router_receiver) -> res => collateral_router.write()
                        .handle_collateral_router_event(res.unwrap())
                        .map_err(|e| eyre!("{:?}", e))
                        .expect("Failed to handle router event"),

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
                    default => break,
                }
            }
            println!("<<< End events\n");
        };

        let heading = |s: &str| {
            println!(
                "    ================================================================| ^^^ {} |==",
                s
            )
        };

        heading(" -> Scenario begins");

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
        heading("Market data sent");

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
        heading("Solver weights sent");

        // wait for solver to solve...
        let solver_weithgs_set = mock_chain_receiver
            .recv_timeout(Duration::from_secs(1))
            .expect("Failed to receive SolverWeightsSet");

        assert!(matches!(
            solver_weithgs_set,
            MockChainInternalNotification::SolverWeightsSet(_, _)
        ));

        chain_connector.write().notify_deposit(
            chain_id,
            get_mock_address_1(),
            collateral_amount,
            timestamp,
        );

        solver_tick(timestamp);
        flush_events();
        heading("Deposit sent");

        fix_server
            .write()
            .notify_server_event(Arc::new(ServerEvent::NewIndexOrder {
                chain_id,
                address: get_mock_address_1(),
                client_order_id: "Order01".into(),
                symbol: get_mock_index_name_1(),
                side: Side::Buy,
                collateral_amount,
                timestamp,
            }));

        flush_events();
        heading("Sent FIX message: NewIndexOrder");

        solver_tick(timestamp);
        flush_events();
        heading("Awaiting collateral");

        timestamp += fund_wait_period;
        solver_tick(timestamp);
        flush_events();
        heading("Clock moved 10 minutes forward");

        for _ in 0..2 {
            let mock_bridge_event = mock_bridge_receiver
                .recv_timeout(Duration::from_secs(1))
                .expect("Failed to receive TransferFunds bridge request");

            assert!(matches!(
                mock_bridge_event,
                MockCollateralBridgeInternalEvent::TransferFunds {
                    chain_id: _,
                    address: _,
                    client_order_id: _,
                    route_from: _,
                    route_to: _,
                    amount: _
                }
            ));
        }

        solver_tick(timestamp);
        flush_events();
        heading("First order batch engaged");

        solver_tick(timestamp);
        flush_events();
        heading("First order batch filled");

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

            println!(" -> FIX response received");
        }

        solver_tick(timestamp);
        flush_events();
        heading("Second order batch engaged");

        solver_tick(timestamp);
        flush_events();
        heading("Second order batch filled");

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

            println!(" -> FIX response received");
        }

        timestamp += fund_wait_period;
        solver_tick(timestamp);
        flush_events();
        heading("Clock moved 10 minutes forward");

        // wait for solver to solve...
        let mint_index = mock_chain_receiver
            .recv_timeout(Duration::from_secs(1))
            .expect("Failed to receive MintIndex");

        assert!(matches!(
            mint_index,
            MockChainInternalNotification::MintIndex {
                chain_id: _,
                symbol: _,
                quantity: _,
                receipient: _,
                execution_price: _,
                execution_time: _
            }
        ));

        println!(" -> Chain response received");

        heading("Scenario completed");
    }
}
