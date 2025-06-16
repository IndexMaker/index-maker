use std::{
    collections::{HashMap, VecDeque},
    sync::{Arc, RwLock as ComponentLock},
};

use chrono::{DateTime, TimeDelta, Utc};
use eyre::{eyre, OptionExt, Result};
use itertools::Itertools;
use parking_lot::{Mutex, RwLock};
use safe_math::safe;

use crate::{
    blockchain::chain_connector::{ChainConnector, ChainNotification},
    collateral::{
        collateral_manager::{CollateralEvent, CollateralManager, CollateralManagerHost},
        collateral_position::{ConfirmStatus, PreAuthStatus},
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
    batch_manager::{BatchEvent, BatchManager, BatchManagerHost},
    index_order_manager::{EngageOrderRequest, IndexOrderEvent, IndexOrderManager},
    index_quote_manager::{QuoteRequestEvent, QuoteRequestManager},
    inventory_manager::{InventoryEvent, InventoryManager},
    solver_order::{SolverClientOrders, SolverOrder, SolverOrderStatus},
    solver_quote::{SolverClientQuotes, SolverQuote, SolverQuoteStatus},
};

pub struct SolverOrderEngagement {
    pub index_order: Arc<RwLock<SolverOrder>>,
    pub asset_contribution_fractions: HashMap<Symbol, Amount>,
    pub asset_quantities: HashMap<Symbol, Amount>,
    pub asset_price_limits: HashMap<Symbol, Amount>,
    pub chain_id: u32,
    pub address: Address,
    pub client_order_id: ClientOrderId,
    pub symbol: Symbol,
    pub basket: Arc<Basket>,
    pub engaged_side: Side,
    pub engaged_collateral: Amount,
    pub new_engaged_collateral: Amount,
    pub engaged_quantity: Amount,
    pub engaged_price: Amount,
    pub filled_quantity: Amount,
}

pub struct EngagedSolverOrders {
    pub batch_order_id: BatchOrderId,
    pub engaged_orders: Vec<SolverOrderEngagement>,
}

pub struct SolveEngagementsResult {
    pub engaged_orders: EngagedSolverOrders,
    pub failed_orders: Vec<Arc<RwLock<SolverOrder>>>,
}

pub struct SolveQuotesResult {
    pub solved_quotes: Vec<Arc<RwLock<SolverQuote>>>,
    pub failed_quotes: Vec<Arc<RwLock<SolverQuote>>>,
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
    fn set_quote_status(&self, order: &mut SolverQuote, status: SolverQuoteStatus);
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

    fn solve_quotes(
        &self,
        strategy_host: &dyn SolverStrategyHost,
        quote_requests: Vec<Arc<RwLock<SolverQuote>>>,
    ) -> Result<SolveQuotesResult>;
}

/// magic solver, needs to take index orders, and based on prices (from price
/// tracker) and available liquiduty (depth from order books), and active orders
/// (from order tracker) calculate best internal-portfolio rebalancing orders,
/// which will (partly) fill (some of the) ordered indexes.  Any position that
/// wasn't matched against ordered indexes shouldn't be kept for too long.
pub struct Solver {
    // solver strategy for calculating order batches
    strategy: Arc<dyn SolverStrategy>,
    basket_manager: Arc<RwLock<BasketManager>>,
    order_id_provider: Arc<RwLock<dyn OrderIdProvider>>,
    price_tracker: Arc<RwLock<PriceTracker>>,
    order_book_manager: Arc<RwLock<dyn OrderBookManager + Send + Sync>>,
    // dependencies
    chain_connector: Arc<ComponentLock<dyn ChainConnector + Send + Sync>>,
    batch_manager: Arc<ComponentLock<BatchManager>>,
    collateral_manager: Arc<ComponentLock<CollateralManager>>,
    index_order_manager: Arc<ComponentLock<IndexOrderManager>>,
    quote_request_manager: Arc<ComponentLock<QuoteRequestManager>>,
    inventory_manager: Arc<RwLock<InventoryManager>>,
    // quotes
    client_quotes: RwLock<SolverClientQuotes>,
    // orders
    client_orders: RwLock<SolverClientOrders>,
    /// A queue with orders that are ready to be engaged, after collateral reached destination
    ready_orders: Mutex<VecDeque<Arc<RwLock<SolverOrder>>>>,
    /// A queue with orders that are ready to be minted
    ready_mints: Mutex<VecDeque<Arc<RwLock<SolverOrder>>>>,
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
        chain_connector: Arc<ComponentLock<dyn ChainConnector + Send + Sync>>,
        batch_manager: Arc<ComponentLock<BatchManager>>,
        collateral_manager: Arc<ComponentLock<CollateralManager>>,
        index_order_manager: Arc<ComponentLock<IndexOrderManager>>,
        quote_request_manager: Arc<ComponentLock<QuoteRequestManager>>,
        inventory_manager: Arc<RwLock<InventoryManager>>,
        max_batch_size: usize,
        zero_threshold: Amount,
        client_order_wait_period: TimeDelta,
        client_quote_wait_period: TimeDelta,
    ) -> Self {
        Self {
            strategy,
            order_id_provider,
            basket_manager,
            price_tracker,
            order_book_manager,
            // dependencies
            chain_connector,
            batch_manager,
            collateral_manager,
            index_order_manager,
            quote_request_manager,
            inventory_manager,
            // quotes
            client_quotes: RwLock::new(SolverClientQuotes::new(client_quote_wait_period)),
            // orders
            client_orders: RwLock::new(SolverClientOrders::new(client_order_wait_period)),
            ready_orders: Mutex::new(VecDeque::new()),
            ready_mints: Mutex::new(VecDeque::new()),
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

    fn handle_failed_orders(
        &self,
        failed_orders: Vec<Arc<RwLock<SolverOrder>>>,
        timestamp: DateTime<Utc>,
    ) -> Result<()> {
        for failed_order in failed_orders {
            let failed_status = failed_order.read().status;
            match failed_status {
                SolverOrderStatus::MissingPrices => {
                    (|o: &SolverOrder| {
                        eprintln!(
                            "(solver) Missing prices for order [{}:{}] {} {}",
                            o.chain_id, o.address, o.client_order_id, o.symbol
                        );
                    })(&failed_order.read());
                    self.client_orders.write().put_back(failed_order);
                }
                _ => {
                    let o = failed_order.read();
                    eprintln!(
                        "(solver) Failed order [{}:{}] {} {} Reason: {:?}",
                        o.chain_id, o.address, o.client_order_id, o.symbol, failed_status
                    );
                    self.index_order_manager
                        .write()
                        .map_err(|e| eyre!("Cannot access index order manager {}", e))?
                        .order_failed(
                            o.chain_id,
                            &o.address,
                            &o.client_order_id,
                            failed_status,
                            timestamp,
                        )?
                }
            }
        }
        Ok(())
    }

    fn engage_more_orders(&self, timestamp: DateTime<Utc>) -> Result<()> {
        let order_batch = self.get_order_batch();

        if order_batch.is_empty() {
            return Ok(());
        }

        let solve_engagements_result = self.strategy.solve_engagements(self, order_batch)?;

        self.handle_failed_orders(solve_engagements_result.failed_orders, timestamp)?;

        if !solve_engagements_result
            .engaged_orders
            .engaged_orders
            .is_empty()
        {
            let mut engaged_orders = solve_engagements_result.engaged_orders;

            // We filter any engagement of negligible size
            engaged_orders
                .engaged_orders
                .retain(|order| self.zero_threshold < order.new_engaged_collateral);

            let send_engage = engaged_orders
                .engaged_orders
                .iter()
                .map(|order| {
                    EngageOrderRequest {
                        chain_id: order.chain_id,
                        address: order.address,
                        client_order_id: order.client_order_id.clone(),
                        symbol: order.symbol.clone(),

                        // We already have engaged collateral that was carried
                        // over from previous batch so we only need to ask Index
                        // Order Manager to engage the difference.
                        collateral_amount: order.new_engaged_collateral,
                    }
                })
                .collect_vec();

            let batch_order_id = self
                .batch_manager
                .write()
                .map_err(|e| eyre!("Failed to access batch manager: {}", e))?
                .handle_new_engagement(Arc::new(RwLock::new(engaged_orders)))?;

            self.index_order_manager
                .write()
                .map_err(|e| eyre!("Failed to access index order manager {}", e))?
                .engage_orders(batch_order_id, send_engage)?;
        }
        Ok(())
    }

    fn manage_collateral(&self, solver_order: Arc<RwLock<SolverOrder>>) -> Result<()> {
        self.set_order_status(
            &mut solver_order.write(),
            SolverOrderStatus::ManageCollateral,
        );

        // Compute amount of collateral required for each asset in the Index basket
        let collateral_management = self
            .strategy
            .query_collateral_management(self, solver_order)?;

        // Manage collateral to have it ready at trading designation(s)
        self.collateral_manager
            .write()
            .map_err(|e| eyre!("Failed to access collateral manager {}", e))?
            .manage_collateral(collateral_management);

        Ok(())
    }

    fn serve_more_clients(&self, timestamp: DateTime<Utc>) -> Result<()> {
        while let Some(solver_order) = self.client_orders.write().get_next_client_order(timestamp) {
            let side = solver_order.read().side;
            match side {
                Side::Buy => self.manage_collateral(solver_order),
                Side::Sell => Err(eyre!("We don't support Sell yet!")),
            }?;
        }
        Ok(())
    }

    fn process_more_quotes(&self, timestamp: DateTime<Utc>) -> Result<()> {
        let mut quote_requests = Vec::new();
        while let Some(solver_quote) = self.client_quotes.write().get_next_client_quote(timestamp) {
            let side = solver_quote.read().side;
            match side {
                Side::Buy => {
                    quote_requests.push(solver_quote);
                    Ok(())
                }
                Side::Sell => Err(eyre!("We don't support Sell yet!")),
            }?;
        }

        let result = self.strategy.solve_quotes(self, quote_requests)?;

        for quote in result.solved_quotes.iter() {
            quote.write().timestamp = timestamp;
        }

        for quote in result.failed_quotes.iter() {
            quote.write().timestamp = timestamp;
        }

        self.quote_request_manager
            .write()
            .map_err(|e| eyre!("Failed to access quote request manager: {:?}", e))?
            .quotes_solved(result)?;

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

        self.collateral_manager
            .write()
            .map_err(|e| eyre!("Failed to access collateral manager: {:?}", e))?
            .confirm_payment(
                index_order.chain_id,
                &index_order.address,
                &index_order.client_order_id,
                &payment_id,
                timestamp,
                index_order.side,
                total_cost,
            )?;

        Ok(())
    }

    fn mint_indexes(&self, timestamp: DateTime<Utc>) -> Result<()> {
        let ready_mints = Vec::from_iter(self.ready_mints.lock().drain(..));

        for mintable_order in ready_mints {
            self.mint_index_order(&mut mintable_order.write(), timestamp)?;
        }

        Ok(())
    }

    /// Core thinking function
    pub fn solve(&self, timestamp: DateTime<Utc>) {
        println!("\n(solver) Begin solve");

        println!("(solver) * Process collateral");
        if let Err(err) = self
            .collateral_manager
            .write()
            .map_err(|e| eyre!("Failed to access collateral manager {}", e))
            .map(|mut x| x.process_collateral(self, timestamp))
        {
            eprintln!("(solver) Error while processing credits: {:?}", err);
        }

        println!("(solver) * Mint indexes");
        if let Err(err) = self.mint_indexes(timestamp) {
            eprintln!("(solver) Error while processing mints: {:?}", err);
        }

        println!("(solver) * Serve more clients");
        if let Err(err) = self.serve_more_clients(timestamp) {
            eprintln!("(solver) Error while serving more clients: {:?}", err);
        }

        println!("(solver) * Engage more orders");
        if let Err(err) = self.engage_more_orders(timestamp) {
            eprintln!("Error while engaging more orders: {:?}", err);
        }

        println!("(solver) * Process batches");
        if let Err(err) = self
            .batch_manager
            .write()
            .map_err(|e| eyre!("Failed to access batch manager {}", e))
            .map(|mut x| x.process_batches(self, timestamp))
        {
            eprintln!("(solver) Error while sending more batches: {:?}", err);
        }

        println!("(solver) * Process quotes");
        if let Err(err) = self.process_more_quotes(timestamp) {
            eprintln!("(solver) Error while processing more quotes: {:?}", err);
        }

        println!("(solver) End solve\n");
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
                .map_err(|e| eyre!("Failed to access collateral manager {}", e))?
                .handle_deposit(self, chain_id, address, amount, timestamp),
            ChainNotification::WithdrawalRequest {
                chain_id,
                address,
                amount,
                timestamp,
            } => self
                .collateral_manager
                .write()
                .map_err(|e| eyre!("Failed to access collateral manager {}", e))?
                .handle_withdrawal(self, chain_id, address, amount, timestamp),
        }
    }

    pub fn handle_batch_event(&self, notification: BatchEvent) -> Result<()> {
        match notification {
            BatchEvent::BatchComplete {
                batch_order_id,
                continued_orders,
            } => {
                println!("(solver) Handle Batch Complete {}", batch_order_id);
                self.ready_orders.lock().extend(continued_orders);
                Ok(())
            }
            BatchEvent::BatchMintable { mintable_orders } => {
                println!("(solver) Handle Batch Mintable");
                self.ready_mints.lock().extend(mintable_orders);
                Ok(())
            }
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

                let order = self.client_orders.read().get_client_order(
                    chain_id,
                    address,
                    client_order_id.clone(),
                );

                if let Some(order) = order {
                    // TODO: Figure out: should collateral manager have already paid for the order?
                    // or CollateralEvent is only to tell us that collateral reached sub-accounts?
                    // NOTE: Paying for order, is just telling collateral manager to block certain
                    // amount of balance, so that any next order from that user won't double-spend.
                    // We assign payment ID so that  we can identify association between order and
                    // allocated collateral.
                    let side = order.read().side;
                    self.collateral_manager
                        .write()
                        .map_err(|e| eyre!("Failed to access collateral manager {}", e))?
                        .preauth_payment(
                            self,
                            chain_id,
                            &address,
                            &client_order_id,
                            timestamp,
                            side,
                            collateral_amount,
                        )?;

                    let symbol = &order.read().symbol;
                    self.index_order_manager
                        .write()
                        .map_err(|e| eyre!("Failed to access index order manager {}", e))?
                        .collateral_ready(
                            chain_id,
                            &address,
                            &client_order_id,
                            symbol,
                            collateral_amount,
                            fee,
                            timestamp,
                        )?;
                }
            }
            CollateralEvent::PreAuthResponse {
                chain_id,
                address,
                client_order_id,
                timestamp: _,
                amount_payable,
                status,
            } => {
                match status {
                    // If we're implementing message based protocol, we should make PaymentApproved
                    // a message that we will receive from collateral manager.
                    PreAuthStatus::Approved { payment_id } => {
                        let order = self.client_orders.read().get_client_order(
                            chain_id,
                            address,
                            client_order_id,
                        );

                        if let Some(order) = order {
                            println!("(solver) PreAuth approved: {}", payment_id);
                            let mut order_write = order.write();
                            order_write
                                .payment_id
                                .replace(payment_id)
                                .is_none()
                                .then_some(())
                                .ok_or_eyre("Payment ID already set")?;

                            if let SolverOrderStatus::Ready = order_write.status {
                                // index order manager has sent back update
                                self.ready_orders.lock().push_back(order.clone());
                            } else {
                                // we're waiting for index order manager
                                self.set_order_status(&mut order_write, SolverOrderStatus::Ready);
                            }
                        } else {
                            eprintln!("(solver) PreAuth approved handling failed: {}", payment_id)
                        }
                    }
                    PreAuthStatus::NotEnoughFunds => {
                        eprintln!(
                            "(solver) PreAuth failed: Not enough funds to pay [{}:{}] {} {:0.5}",
                            chain_id, address, client_order_id, amount_payable
                        )
                    }
                }
            }
            CollateralEvent::ConfirmResponse {
                chain_id,
                address,
                client_order_id,
                payment_id,
                timestamp,
                amount_paid,
                status,
            } => match status {
                ConfirmStatus::Authorized => {
                    let order = self.client_orders.read().get_client_order(
                        chain_id,
                        address,
                        client_order_id.clone(),
                    );

                    if let Some(order) = order {
                        println!("(solver) Payment authorized: {}", payment_id);
                        let mut order_write = order.write();
                        self.chain_connector
                            .write()
                            .map_err(|e| eyre!("Failed to access chain connector {}", e))?
                            .mint_index(
                                order_write.chain_id,
                                order_write.symbol.clone(),
                                order_write.filled_quantity,
                                order_write.address,
                                amount_paid,
                                timestamp,
                            );

                        let lots = order_write.lots.drain(..).collect_vec();
                        self.index_order_manager
                            .write()
                            .map_err(|e| eyre!("Failed to access index order manager {}", e))?
                            .order_request_minted(
                                chain_id,
                                &address,
                                &client_order_id,
                                &order_write.symbol,
                                &payment_id,
                                amount_paid,
                                lots,
                                timestamp,
                            )?;

                        self.set_order_status(&mut order_write, SolverOrderStatus::Minted);
                    } else {
                        eprintln!(
                            "(solver) Payment authorized handling failed: {}",
                            payment_id
                        )
                    }
                }
                ConfirmStatus::NotEnoughFunds => {
                    eprintln!(
                        "(solver) Payment failed: Not enough funds to pay [{}:{}] {} {}",
                        chain_id, address, client_order_id, payment_id
                    )
                }
            },
        }
        Ok(())
    }

    /// receive Index Order
    pub fn handle_index_order(&self, notification: IndexOrderEvent) -> Result<()> {
        match notification {
            IndexOrderEvent::NewIndexOrder {
                chain_id,
                address,
                client_order_id,
                symbol,
                side,
                collateral_amount,
                timestamp,
            } => {
                println!(
                    "\n(solver) Handle Index Order NewIndexOrder {} {} < {} from {}",
                    symbol, client_order_id, client_order_id, address
                );
                self.client_orders.write().add_client_order(
                    chain_id,
                    address,
                    client_order_id,
                    symbol,
                    side,
                    collateral_amount,
                    timestamp,
                )
            }
            IndexOrderEvent::CancelIndexOrder {
                chain_id,
                address,
                client_order_id,
                timestamp: _,
            } => {
                println!(
                    "\n(solver) Handle Cancel Index Order [{}:{}] {}",
                    chain_id, address, client_order_id
                );
                self.client_orders
                    .write()
                    .cancel_client_order(chain_id, address, client_order_id)
            }
            IndexOrderEvent::UpdateIndexOrder {
                chain_id,
                address,
                client_order_id,
                collateral_removed,
                collateral_remaining: _,
                timestamp,
            } => {
                println!(
                    "\n(solver) Handle Index Order UpdateIndexOrder {} < {} from {}",
                    client_order_id, client_order_id, address
                );
                self.client_orders.write().update_client_order(
                    chain_id,
                    address,
                    client_order_id,
                    collateral_removed,
                    timestamp,
                )
            }
            IndexOrderEvent::EngageIndexOrder {
                batch_order_id,
                engaged_orders,
                timestamp,
            } => self
                .batch_manager
                .write()
                .map_err(|e| eyre!("Failed to access batch manager {}", e))?
                .handle_engage_index_order(self, batch_order_id, engaged_orders, timestamp),
            IndexOrderEvent::CollateralReady {
                chain_id,
                address,
                client_order_id,
                collateral_remaining,
                collateral_spent,
                fees: _,
                timestamp,
            } => {
                println!(
                    "\n(solver) Handle Index Order CollateralReady {} < {} from {}: {:0.5} {:0.5}",
                    chain_id, client_order_id, address, collateral_remaining, collateral_spent
                );
                if let Some(order) =
                    self.client_orders
                        .read()
                        .get_client_order(chain_id, address, client_order_id)
                {
                    let mut order_write = order.write();
                    order_write.collateral_spent = collateral_spent;
                    order_write.remaining_collateral = collateral_remaining;
                    order_write.timestamp = timestamp;

                    if let SolverOrderStatus::Ready = order_write.status {
                        // collateral manager has sent back pre-auth
                        self.ready_orders.lock().push_back(order.clone());
                    } else {
                        // we're waiting for colalteral manager
                        self.set_order_status(&mut order_write, SolverOrderStatus::Ready);
                    }

                    Ok(())
                } else {
                    // TODO: Tell CollateralManager that order is no longer
                    // Something needs to happen with collateral, e.g. reuse in next order
                    Err(eyre!("(solver) Handle collateral ready ack: Missing order"))
                }
            }
        }
    }

    // receive QR
    pub fn handle_quote_request(&self, notification: QuoteRequestEvent) -> Result<()> {
        println!("\n(solver) Handle Quote Request");
        match notification {
            QuoteRequestEvent::NewQuoteRequest {
                chain_id,
                address,
                client_quote_id,
                symbol,
                side,
                collateral_amount,
                timestamp,
            } => self.client_quotes.write().add_client_quote(
                chain_id,
                address,
                client_quote_id,
                symbol,
                side,
                collateral_amount,
                timestamp,
            ),
            QuoteRequestEvent::CancelQuoteRequest {
                chain_id,
                address,
                client_quote_id,
                timestamp: _,
            } => self
                .client_quotes
                .write()
                .cancel_client_order(chain_id, address, client_quote_id),
        }
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
                    "\n(solver) Handle Inventory Event OpenLot {} {:?} {:5} {:0.5} @ {:0.5} + fee {:0.5} ({:0.3}%)",
                    lot_id,
                    side,
                    symbol,
                    quantity,
                    price,
                    fee,
                    (|| safe!(safe!(fee * Amount::ONE_HUNDRED) / safe!(quantity * price)?))().unwrap_or_default()
                );
                self.batch_manager
                    .read()
                    .map_err(|e| eyre!("Failed to access batch manager {}", e))?
                    .handle_new_lot(
                        self,
                        order_id,
                        batch_order_id,
                        lot_id,
                        None,
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
                original_lot_id,
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
                    "\n(solver) Handle Inventory Event CloseLot {} {} {:?} {:5} {:0.5}@{:0.5}+{:0.5} ({:0.5}%)",
                    original_lot_id,
                    closing_lot_id,
                    side,
                    symbol,
                    quantity_closed,
                    closing_price,
                    closing_fee,
                    (|| safe!(safe!(Amount::ONE_HUNDRED * safe!(original_quantity - quantity_remaining)?)?
                        / original_quantity))().unwrap_or_default()
                );
                self.batch_manager
                    .read()
                    .map_err(|e| eyre!("Failed to access batch manager {}", e))?
                    .handle_new_lot(
                        self,
                        closing_order_id,
                        closing_batch_order_id,
                        original_lot_id,
                        Some(closing_lot_id),
                        symbol,
                        side,
                        closing_price,
                        quantity_closed,
                        closing_fee,
                        closing_timestamp,
                    )
            }
            InventoryEvent::Cancel {
                order_id,
                batch_order_id,
                symbol,
                side,
                quantity_cancelled,
                original_quantity: _,
                quantity_remaining: _,
                is_cancelled,
                cancel_timestamp,
            } => {
                println!(
                    "(solver) Handle Inventory Event Cancel {} {} {}",
                    order_id, batch_order_id, symbol
                );
                self.batch_manager
                    .read()
                    .map_err(|e| eyre!("Failed to access batch manager {}", e))?
                    .handle_cancel_order(
                        self,
                        batch_order_id,
                        symbol,
                        side,
                        quantity_cancelled,
                        is_cancelled,
                        cancel_timestamp,
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
    pub fn handle_basket_event(&self, notification: BasketNotification) -> Result<()> {
        // TODO: (move this) once solvign is done notify new weights were applied
        match notification {
            BasketNotification::BasketAdded(symbol, basket) => {
                println!("(solver) Handle Basket Notification BasketAdded {}", symbol);
                self.chain_connector
                    .write()
                    .map_err(|e| eyre!("Failed to access chain connector {}", e))?
                    .solver_weights_set(symbol, basket);
                Ok(())
            }
            BasketNotification::BasketUpdated(symbol, basket) => {
                println!(
                    "(solver) Handle Basket Notification BasketUpdated {}",
                    symbol
                );
                self.chain_connector
                    .write()
                    .map_err(|e| eyre!("Failed to access chain connector {}", e))?
                    .solver_weights_set(symbol, basket);
                Ok(())
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

    fn set_quote_status(&self, order: &mut SolverQuote, status: SolverQuoteStatus) {
        println!(
            "(solver) Set Index Quote Status: {} {:?}",
            order.client_quote_id, status
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

    fn send_order_batch(&self, batch_order: Arc<BatchOrder>) -> Result<()> {
        self.inventory_manager.write().new_order_batch(batch_order)
    }

    fn fill_order_request(
        &self,
        chain_id: u32,
        address: &Address,
        client_order_id: &ClientOrderId,
        symbol: &Symbol,
        collateral_spent: Amount,
        fill_amount: Amount,
        timestamp: DateTime<Utc>,
    ) -> Result<()> {
        self.index_order_manager
            .write()
            .map_err(|e| eyre!("Failed to access index order manager {}", e))?
            .fill_order_request(
                chain_id,
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
    use std::{
        sync::{Arc, RwLock as ComponentLock},
        time::Duration,
    };

    use chrono::{TimeDelta, Utc};
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
                IntoObservableMany, IntoObservableSingle
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
            order_connector::{test_util::MockOrderConnector, OrderConnectorNotification, SessionId},
            order_tracker::{OrderTracker, OrderTrackerNotification},
        },
        server::server::{test_util::MockServer, ServerEvent, ServerResponse},
        solver::{position::LotId, solvers::simple_solver::SimpleSolver},
    };

    use super::*;

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
        let mint_wait_period = TimeDelta::new(10, 0).unwrap();
        let client_order_wait_period = TimeDelta::new(10, 0).unwrap();
        let client_quote_wait_period = TimeDelta::new(1, 0).unwrap();

        let (chain_sender, chain_receiver) = unbounded::<ChainNotification>();
        let (collateral_sender, collateral_receiver) = unbounded::<CollateralEvent>();
        let (index_order_sender, index_order_receiver) = unbounded::<IndexOrderEvent>();
        let (quote_request_sender, quote_request_receiver) = unbounded::<QuoteRequestEvent>();
        let (inventory_sender, inventory_receiver) = unbounded::<InventoryEvent>();
        let (book_sender, book_receiver) = unbounded::<OrderBookEvent>();
        let (price_sender, price_receiver) = unbounded::<PriceEvent>();
        let (market_sender, market_receiver) = unbounded::<Arc<MarketDataEvent>>();
        let (basket_sender, basket_receiver) = unbounded::<BasketNotification>();
        let (batch_event_sender, batch_event_receiver) = unbounded::<BatchEvent>();
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

        let chain_connector = Arc::new(ComponentLock::new(MockChainConnector::new()));
        let fix_server = Arc::new(RwLock::new(MockServer::new()));

        let collateral_designation_1 = Arc::new(ComponentLock::new(MockCollateralDesignation {
            type_: "T1".into(),
            name: "D1".into(),
            collateral_symbol: "C1".into(),
            full_name: "T1:D1:C1".into(),
            balance: dec!(0.0),
        }));

        let collateral_designation_2 = Arc::new(ComponentLock::new(MockCollateralDesignation {
            type_: "T2".into(),
            name: "D2".into(),
            collateral_symbol: "C2".into(),
            full_name: "T2:D2:C2".into(),
            balance: dec!(0.0),
        }));

        let collateral_designation_3 = Arc::new(ComponentLock::new(MockCollateralDesignation {
            type_: "T3".into(),
            name: "D3".into(),
            collateral_symbol: "C3".into(),
            full_name: "T3:D3:C3".into(),
            balance: dec!(0.0),
        }));

        let collateral_bridge_1 = Arc::new(ComponentLock::new(MockCollateralBridge::new(
            collateral_designation_1.clone(),
            collateral_designation_2.clone(),
        )));

        let collateral_bridge_2 = Arc::new(ComponentLock::new(MockCollateralBridge::new(
            collateral_designation_2.clone(),
            collateral_designation_3.clone(),
        )));

        let collateral_router = Arc::new(ComponentLock::new(CollateralRouter::new()));

        collateral_router
            .write()
            .unwrap()
            .add_bridge(collateral_bridge_1.clone())
            .expect("Failed to add bridge");

        collateral_router
            .write()
            .unwrap()
            .add_bridge(collateral_bridge_2.clone())
            .expect("Failed to add bridge");

        collateral_router
            .write()
            .unwrap()
            .add_chain_source(
                chain_id,
                collateral_designation_1
                    .read()
                    .unwrap()
                    .get_full_name()
                    .clone(),
            )
            .expect("Failed to add chain source");

        collateral_router
            .write()
            .unwrap()
            .set_default_destination(
                collateral_designation_3
                    .read()
                    .unwrap()
                    .get_full_name()
                    .clone(),
            )
            .expect("Failed to set default destination");

        collateral_router
            .write()
            .unwrap()
            .add_route(&[
                collateral_designation_1
                    .read()
                    .unwrap()
                    .get_full_name()
                    .clone(),
                collateral_designation_2
                    .read()
                    .unwrap()
                    .get_full_name()
                    .clone(),
                collateral_designation_3
                    .read()
                    .unwrap()
                    .get_full_name()
                    .clone(),
            ])
            .expect("Failed to add route");

        let collateral_manager = Arc::new(ComponentLock::new(CollateralManager::new(
            collateral_router.clone(),
            tolerance,
        )));

        let index_order_manager = Arc::new(ComponentLock::new(IndexOrderManager::new(
            fix_server.clone(),
            tolerance,
        )));
        let quote_request_manager = Arc::new(ComponentLock::new(QuoteRequestManager::new(
            fix_server.clone(),
        )));

        let basket_manager = Arc::new(RwLock::new(BasketManager::new()));

        let order_id_provider = Arc::new(RwLock::new(MockOrderIdProvider {
            order_ids: VecDeque::from_iter((1..7).map(|n| OrderId(format!("O-{:02}", n)))),
            batch_order_ids: VecDeque::from_iter(
                (1..4).map(|n| BatchOrderId(format!("B-{:02}", n))),
            ),
            payment_ids: VecDeque::from_iter((1..4).map(|n| PaymentId(format!("P-{:02}", n)))),
        }));

        let solver_strategy = Arc::new(SimpleSolver::new(
            dec!(0.01),
            dec!(1.001),
            dec!(3000.0),
            dec!(2000.0),
        ));

        let batch_manager = Arc::new(ComponentLock::new(BatchManager::new(
            max_batch_size,
            tolerance,
            dec!(0.9999),
            dec!(0.99),
            mint_wait_period,
        )));

        let solver = Arc::new(Solver::new(
            solver_strategy.clone(),
            order_id_provider.clone(),
            basket_manager.clone(),
            price_tracker.clone(),
            order_book_manager.clone(),
            chain_connector.clone(),
            batch_manager.clone(),
            collateral_manager.clone(),
            index_order_manager.clone(),
            quote_request_manager.clone(),
            inventory_manager.clone(),
            max_batch_size,
            tolerance,
            client_order_wait_period,
            client_quote_wait_period,
        ));

        solver
            .basket_manager
            .write()
            .get_single_observer_mut()
            .set_observer_from(basket_sender);

        chain_connector
            .write()
            .unwrap()
            .get_single_observer_mut()
            .set_observer_from(chain_sender);

        batch_manager
            .write()
            .unwrap()
            .get_single_observer_mut()
            .set_observer_from(batch_event_sender);

        collateral_manager
            .write()
            .unwrap()
            .get_single_observer_mut()
            .set_observer_from(collateral_sender);

        collateral_bridge_1
            .write()
            .unwrap()
            .get_single_observer_mut()
            .set_observer_from(collateral_router_sender.clone());

        collateral_bridge_2
            .write()
            .unwrap()
            .get_single_observer_mut()
            .set_observer_from(collateral_router_sender);

        collateral_router
            .write()
            .unwrap()
            .get_single_observer_mut()
            .set_observer_from(collateral_transfer_sender);

        index_order_manager
            .write()
            .unwrap()
            .get_single_observer_mut()
            .set_observer_from(index_order_sender);

        quote_request_manager
            .write()
            .unwrap()
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

        let lot_ids = RwLock::new(VecDeque::<LotId>::from_iter(
            (1..13).map(|n| LotId(format!("L-{:02}", n))),
        ));
        let order_connector_weak = Arc::downgrade(&order_connector);
        let (defer_1, deferred) = unbounded::<Box<dyn FnOnce() + Send + Sync>>();
        let defer_2 = defer_1.clone();
        let fill_pattern = Arc::new(Mutex::new(VecDeque::from([
            (dec!(0.3), dec!(0.1)),
            (dec!(0.1), dec!(0.3)),
        ])));
        order_connector
            .write()
            .implementor
            .set_observer_fn(move |(sid, e): (SessionId, Arc<SingleOrder>)| {
                let order_connector = order_connector_weak.upgrade().unwrap();
                let lot_id_1 = lot_ids.write().pop_front().unwrap();
                let lot_id_2 = lot_ids.write().pop_front().unwrap();
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
                let q1 = e.quantity * dec!(0.6);
                let (f2, f3) = fill_pattern
                    .lock()
                    .pop_front()
                    .unwrap_or((dec!(0.4), Amount::ZERO));
                let (q2, q3) = (e.quantity * f2, e.quantity * f3);
                let defer = defer_1.clone();
                println!(
                    "(mock) SingleOrder {} {:0.5} @ {:0.5} {:0.5} @ {:0.5}",
                    e.symbol, q1, p1, q2, p2
                );
                // Note we defer first fill to make sure we don't get dead-lock
                defer_1
                    .send(Box::new(move || {
                        order_connector.write().notify_fill(
                            e.order_id.clone(),
                            lot_id_1.clone(),
                            e.symbol.clone(),
                            e.side,
                            p1,
                            q1,
                            dec!(0.001) * p1 * q1,
                            e.created_timestamp,
                        );
                        let defer_ = defer.clone();
                        // We defer second fill, so that fills of different orders
                        // will be interleaved. We do that to test progressive fill-rate
                        // of the Index Order in our simulation.
                        defer
                            .send(Box::new(move || {
                                order_connector.write().notify_fill(
                                    e.order_id.clone(),
                                    lot_id_2.clone(),
                                    e.symbol.clone(),
                                    e.side,
                                    p2,
                                    q2,
                                    dec!(0.001) * p2 * q2,
                                    e.created_timestamp,
                                );
                                defer_
                                    .send(Box::new(move || {
                                        order_connector.write().notify_cancel(
                                            e.order_id.clone(),
                                            e.symbol.clone(),
                                            e.side,
                                            q3,
                                            e.created_timestamp,
                                        );
                                    }))
                                    .unwrap();
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
            .unwrap()
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
                        chain_id,
                        address,
                        client_order_id,
                        timestamp,
                    } => {
                        println!(
                            "(mock) FIX Order Response: ACK [{}:{}] {} {}",
                            chain_id, address, client_order_id, timestamp
                        );
                    }
                    ServerResponse::IndexOrderFill {
                        chain_id,
                        address,
                        client_order_id,
                        filled_quantity,
                        collateral_remaining,
                        collateral_spent,
                        timestamp,
                    } => {
                        println!(
                            "(mock) FIX Order Response: EXE [{}:{}] {} {:0.5} {:0.5} {:0.5} {}",
                            chain_id,
                            address,
                            client_order_id,
                            filled_quantity,
                            collateral_remaining,
                            collateral_spent,
                            timestamp
                        );
                    }
                    ServerResponse::NewIndexQuoteAck {
                        chain_id,
                        address,
                        client_quote_id,
                        timestamp,
                    } => {
                        println!(
                            "(mock) FIX Quote Response: ACK [{}:{}] {} {}",
                            chain_id, address, client_quote_id, timestamp
                        );
                    }
                    ServerResponse::IndexQuoteResponse {
                        chain_id,
                        address,
                        client_quote_id,
                        quantity_possible,
                        timestamp,
                    } => {
                        println!(
                            "(mock) FIX Quote Response: EXE [{}:{}] {} {:0.5} {}",
                            chain_id, address, client_quote_id, quantity_possible, timestamp
                        );
                    }
                    response => {
                        assert!(false, "Unexpected response type {:?}", response);
                    }
                };
                mock_fix_sender
                    .send(response)
                    .expect("Failed to send FIX response");
                println!("(mock) FIX response sent");
            });

        let impl_collateral_bridge =
            move |collateral_bridge: &Arc<ComponentLock<MockCollateralBridge>>,
                  mock_bridge_sender: Sender<MockCollateralBridgeInternalEvent>,
                  defer_2: Sender<Box<dyn FnOnce() + Send + Sync>>| {
                let collateral_bridge_weak = Arc::downgrade(collateral_bridge);
                collateral_bridge
                    .write()
                    .unwrap()
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
                                cumulative_fee,
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
                                let cumulative_fee = cumulative_fee + fee;
                                let timestamp = Utc::now();
                                defer_2
                                    .send(Box::new(move || {
                                        collateral_bridge
                                            .write()
                                            .unwrap()
                                            .notify_collateral_router_event(
                                                chain_id,
                                                address,
                                                client_order_id,
                                                timestamp,
                                                route_from,
                                                route_to,
                                                amount - fee,
                                                cumulative_fee,
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
            };

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
                    recv(price_receiver) -> res => solver.handle_price_event(res.unwrap()),
                    recv(book_receiver) -> res => solver.handle_book_event(res.unwrap()),
                    recv(basket_receiver) -> res => solver.handle_basket_event(res.unwrap())
                        .map_err(|e| eyre!("{:?}", e))
                        .expect("Failed to handle basket event"),

                    recv(chain_receiver) -> res => solver.handle_chain_event(res.unwrap())
                        .map_err(|e| eyre!("{:?}", e))
                        .expect("Failed to handle chain event"),

                    recv(batch_event_receiver) -> res => solver.handle_batch_event(res.unwrap())
                        .map_err(|e| eyre!("{:?}", e))
                        .expect("Failed to handle batch_event event"),

                    recv(collateral_receiver) -> res => solver.handle_collateral_event(res.unwrap())
                        .map_err(|e| eyre!("{:?}", e))
                        .expect("Failed to handle collateral event"),

                    recv(collateral_transfer_receiver) -> res => solver.collateral_manager.write()
                        .unwrap()
                        .handle_collateral_transfer_event(res.unwrap())
                        .map_err(|e| eyre!("{:?}", e))
                        .expect("Failed to handle bridge event"),

                    recv(collateral_router_receiver) -> res => collateral_router.write()
                        .unwrap()
                        .handle_collateral_router_event(res.unwrap())
                        .map_err(|e| eyre!("{:?}", e))
                        .expect("Failed to handle router event"),

                    recv(inventory_receiver) -> res => solver.handle_inventory_event(res.unwrap())
                        .map_err(|e| eyre!("{:?}", e))
                        .expect("Failed to handle inventory event"),

                    recv(index_order_receiver) -> res => solver.handle_index_order(res.unwrap())
                        .map_err(|e| eyre!("{:?}", e))
                        .expect("Failed to handle index order manager event"),

                    recv(quote_request_receiver) -> res => solver.handle_quote_request(res.unwrap())
                        .map_err(|e| eyre!("{:?}", e))
                        .expect("Failed to handle index quote manager event"),

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
                            .unwrap()
                            .handle_server_message(&e)
                            .map_err(|e| eyre!("{:?}", e))
                            .expect("Failed to handle server event");

                        quote_request_manager
                            .write()
                            .unwrap()
                            .handle_server_message(&e)
                            .map_err(|e| eyre!("{:?}", e))
                            .expect("Failed to handle quote request event");
                    },
                    recv(order_tracker_receiver) -> res => {
                        inventory_manager
                            .write()
                            .handle_fill_report(res.unwrap())
                            .map_err(|e| eyre!("{:?}", e))
                            .expect("Failed to handle order tracker event");
                    },
                    recv(order_connector_receiver) -> res => {
                        order_tracker
                            .write()
                            .handle_order_notification(res.unwrap())
                            .map_err(|e| eyre!("{:?}", e))
                            .expect("Failed to handle order connector event");
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
        order_connector.write().notify_logon("Session-01".into());

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
            1,
            dec!(90.0),
            dec!(100.0),
            dec!(10.0),
            dec!(20.0),
        );

        market_data_connector.write().notify_top_of_book(
            get_mock_asset_name_2(),
            2,
            dec!(295.0),
            dec!(300.0),
            dec!(80.0),
            dec!(50.0),
        );

        // last trade
        market_data_connector
            .write()
            .notify_trade(get_mock_asset_name_1(), 3, dec!(90.0), dec!(5.0));

        market_data_connector.write().notify_trade(
            get_mock_asset_name_2(),
            4,
            dec!(300.0),
            dec!(15.0),
        );

        // book depth
        market_data_connector.write().notify_full_order_book(
            get_mock_asset_name_1(),
            5,
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
            6,
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
            .unwrap()
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

        fix_server
            .write()
            .notify_server_event(Arc::new(ServerEvent::NewQuoteRequest {
                chain_id,
                address: get_mock_address_1(),
                client_quote_id: "Q-01".into(),
                symbol: get_mock_index_name_1(),
                side: Side::Buy,
                collateral_amount,
                timestamp,
            }));

        solver_tick(timestamp);
        flush_events();
        heading("Sent FIX message: NewQuoteRequest");

        timestamp += client_quote_wait_period;
        solver_tick(timestamp);
        flush_events();
        heading(
            format!(
                "Clock moved forward {:0.1}s",
                client_quote_wait_period.as_seconds_f32()
            )
            .as_str(),
        );

        let fix_response = mock_fix_receiver
            .recv_timeout(Duration::from_secs(1))
            .expect("Failed to receive ServerResponse");

        assert!(matches!(
            fix_response,
            ServerResponse::NewIndexQuoteAck {
                chain_id: _,
                address: _,
                client_quote_id: _,
                timestamp: _
            }
        ));

        flush_events();

        let fix_response = mock_fix_receiver
            .recv_timeout(Duration::from_secs(1))
            .expect("Failed to receive ServerResponse");

        assert!(matches!(
            fix_response,
            ServerResponse::IndexQuoteResponse {
                chain_id: _,
                address: _,
                client_quote_id: _,
                quantity_possible: _,
                timestamp: _
            }
        ));

        chain_connector.write().unwrap().notify_deposit(
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
                client_order_id: "C-01".into(),
                symbol: get_mock_index_name_1(),
                side: Side::Buy,
                collateral_amount,
                timestamp,
            }));

        flush_events();
        heading("Sent FIX message: NewIndexOrder");

        timestamp += client_order_wait_period;
        solver_tick(timestamp);
        flush_events();
        heading(
            format!(
                "Clock moved forward {:0.1}s",
                client_order_wait_period.as_seconds_f32()
            )
            .as_str(),
        );

        solver_tick(timestamp);
        flush_events();
        heading("Awaiting collateral");

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
                    amount: _,
                    cumulative_fee: _
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
                chain_id: _,
                address: _,
                client_order_id: _,
                timestamp: _
            }
        ));

        let order1 = order_tracker_2.read().get_order(&"O-01".into());
        let order2 = order_tracker_2.read().get_order(&"O-02".into());

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
                    chain_id: _,
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

        for _ in 0..2 {
            solver_tick(timestamp);
            flush_events();
            heading("Next order batch engaged");

            solver_tick(timestamp);
            flush_events();
            heading("Next order batch filled");

            for _ in 0..2 {
                let fix_response = mock_fix_receiver
                    .recv_timeout(Duration::from_secs(1))
                    .expect("Failed to receive ServerResponse");

                assert!(matches!(
                    fix_response,
                    ServerResponse::IndexOrderFill {
                        chain_id: _,
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
        }

        flush_events();

        timestamp += mint_wait_period;
        solver_tick(timestamp);
        flush_events();
        heading(
            format!(
                "Clock moved forward {:0.1}s",
                mint_wait_period.as_seconds_f32()
            )
            .as_str(),
        );

        solver_tick(timestamp);
        flush_events();

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

        order_connector.write().notify_logout("Session-01".into());
        heading("Scenario completed");
    }
}
