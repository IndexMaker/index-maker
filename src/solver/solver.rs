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
            Address, Amount, AssetOrder, BatchOrder, BatchOrderId, ClientOrderId, OrderId,
            PaymentId, PriceType, Side, Symbol,
        },
        decimal_ext::DecimalExt,
    },
    index::{
        basket::Basket,
        basket_manager::{BasketManager, BasketNotification},
    },
    market_data::{
        order_book::order_book_manager::{OrderBookEvent, OrderBookManager},
        price_tracker::{PriceEvent, PriceTracker},
    },
};

use super::{
    index_order_manager::{IndexOrderEvent, IndexOrderManager},
    index_quote_manager::{QuoteRequestEvent, QuoteRequestManager},
    inventory_manager::{InventoryEvent, InventoryManager},
    position::LotId,
};

#[derive(Clone, Copy, Debug)]
pub enum SolverOrderStatus {
    Open,
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
    fn try_new(symbol: Symbol, quantity: Amount, lot: &BatchAssetLot) -> Option<Self> {
        Some(Self {
            symbol,
            quantity,
            price: lot.price,
            fee: safe!(lot.fee * safe!(quantity / lot.original_quantity)?)?,
        })
    }
}

/// Every execution of any order in the batch produces a lot of the asset on the
/// order.
///
/// Note that InventoryManager also manages lots, however these are lots in our
/// inventory, for us to know what we've got in the inventory and how we
/// acquired it. The BatchAssetLot is different kind of lot, in which we store
/// one unit of execution for a batch created by Solver, and then we allocate
/// some portion of that unit to index orders, using allocations list.
struct BatchAssetLot {
    /// Asset Order ID
    order_id: OrderId,

    /// Transaction ID
    lot_id: LotId,

    /// Quantity filled
    original_quantity: Amount,

    /// Unallocated quantity
    remaining_quantity: Amount,

    /// Executed price
    price: Amount,

    /// Execution fee
    fee: Amount,

    /// Timestamp of execution
    timestamp: DateTime<Utc>,
}

/// This is aggregated position on asset within the batch.
///
/// Note that InventoryManager also manages positions, however these are
/// positions in out inventory, for us to know what is our absolute position at
/// any given moment. The BatchAssetPosition is different kind of position, in
/// which we aggregate position over all lots for a batch created by Solver.
struct BatchAssetPosition {
    /// Symbol of an asset
    symbol: Symbol,

    /// Side of an order
    side: Side,

    /// Position in this batch of this asset on that side
    /// Note: A batch can have both Buy and Sell orders,
    /// and we need separate position for them, as these
    /// will be matched for different users.
    position: Amount,

    /// Total quantity on all orders for this asset on that side
    order_quantity: Amount,

    /// Total price we paid for reaching the position
    realized_value: Amount,

    /// Total price we intended to pay on the order
    volley_size: Amount,

    /// Total fee we paid on the order
    fee: Amount,

    /// Time of the last transaction of this asset on this side
    last_update_timestamp: DateTime<Utc>,

    /// Unallocated lots
    open_lots: VecDeque<BatchAssetLot>,

    /// Allocated lots
    closed_lots: VecDeque<BatchAssetLot>,
}

impl BatchAssetPosition {
    pub fn try_new(order: &AssetOrder, timestamp: DateTime<Utc>) -> Option<Self> {
        Some(Self {
            symbol: order.symbol.clone(),
            side: order.side,
            order_quantity: order.quantity,
            volley_size: safe!(order.price * order.quantity)?,
            position: Amount::ZERO,
            realized_value: Amount::ZERO,
            fee: Amount::ZERO,
            last_update_timestamp: timestamp,
            open_lots: VecDeque::new(),
            closed_lots: VecDeque::new(),
        })
    }

    pub fn try_allocate_lots(
        &mut self,
        index_order: &mut SolverOrder,
        mut quantity: Amount,
    ) -> Option<Amount> {
        let mut push_allocation = |lot: &mut BatchAssetLot, quantity: Amount| -> Option<()> {
            let asset_allocation =
                SolverOrderAssetLot::try_new(self.symbol.clone(), quantity, lot)?;
            index_order.lots.push(asset_allocation);
            Some(())
        };
        while let Some(lot) = self.open_lots.front_mut() {
            if lot.remaining_quantity < quantity {
                let mut lot = self
                    .open_lots
                    .pop_front()
                    .expect("Should have front at this stage");
                let used_quantity = lot.remaining_quantity;
                push_allocation(&mut lot, used_quantity)?;
                quantity = safe!(quantity - used_quantity)?;
                lot.remaining_quantity = Amount::ZERO;
                self.closed_lots.push_back(lot);
            } else {
                push_allocation(lot, quantity)?;
                lot.remaining_quantity = safe!(lot.remaining_quantity - quantity)?;
                quantity = Amount::ZERO;
                break;
            }
        }
        Some(quantity)
    }
}

struct BatchOrderStatus {
    // ID of the batch order
    batch_order_id: BatchOrderId,

    /// Positions of individual assets in this batch
    /// Note: These aren't our absolute positions, these are only positions
    /// of assets acquired/disposed in this batch
    positions: HashMap<(Symbol, Side), BatchAssetPosition>,

    /// Volley size (value of all orders in the batch)
    ///
    /// Note: We choose term "volley" and not "value" here:
    ///
    /// - Value: Carries the connotation of something you aim to preserve, hold
    ///     onto, or realize in a lasting way.
    ///
    /// - Volley: Implies a temporary burst, a collection meant to be processed
    ///     and then resolved or dispersed. It suggests an intent to move through a
    ///     state and then be "gotten rid of" in its initial form (either by
    ///     execution, cancellation, or the batch expiring)
    ///
    /// Then we will have total_volley_size across all batches, and for rate-limit
    /// we will have max_volley_size.
    ///
    volley_size: Amount,

    /// Filled volley (value of all fills across all orders in the batch)
    filled_volley: Amount,

    /// Fill-rate of this batch as a whole
    filled_fraction: Amount,

    /// Total price we paid for reaching the positions
    realized_value: Amount,

    /// Total fee we paid on the batch
    fee: Amount,

    /// Time of the last transaction
    last_update_timestamp: DateTime<Utc>,
}

impl BatchOrderStatus {
    pub fn new(batch_order_id: BatchOrderId, timestamp: DateTime<Utc>) -> Self {
        Self {
            batch_order_id,
            positions: HashMap::new(),
            volley_size: Amount::ZERO,
            filled_volley: Amount::ZERO,
            filled_fraction: Amount::ZERO,
            realized_value: Amount::ZERO,
            fee: Amount::ZERO,
            last_update_timestamp: timestamp,
        }
    }

    pub fn try_add_position(&mut self, order: &AssetOrder, timestamp: DateTime<Utc>) -> Option<()> {
        let key = (order.symbol.clone(), order.side);
        match self.positions.entry(key) {
            Entry::Occupied(mut entry) => {
                let position = entry.get_mut();
                position.order_quantity = safe!(position.order_quantity + order.quantity)?;
            }
            Entry::Vacant(entry) => {
                let position = BatchAssetPosition::try_new(order, timestamp)?;
                self.volley_size = safe!(self.volley_size + position.volley_size)?;
                entry.insert(position);
            }
        }
        Some(())
    }
}

struct CollateralTransaction {
    payment_id: PaymentId,
    amount: Amount,
    timestamp: DateTime<Utc>,
}

struct CollateralPosition {
    /// On-chain address of the User
    address: Address,

    /// Total balance of credits less debits in force
    balance: Amount,

    /// Total amount of pending credits
    pending_cr: Amount,

    /// Total amount of pending debits
    pending_dr: Amount,

    /// Pending credits
    transactions_cr: VecDeque<CollateralTransaction>,

    /// Pending debits
    transactions_dr: VecDeque<CollateralTransaction>,

    /// Completed credits and debits
    completed_transactions: VecDeque<CollateralTransaction>,

    /// Last time we updated this position
    last_update_timestamp: DateTime<Utc>,
}

impl CollateralPosition {
    fn new(address: Address) -> Self {
        Self {
            address,
            balance: Amount::ZERO,
            pending_cr: Amount::ZERO,
            pending_dr: Amount::ZERO,
            transactions_cr: VecDeque::new(),
            transactions_dr: VecDeque::new(),
            completed_transactions: VecDeque::new(),
            last_update_timestamp: Default::default(),
        }
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

pub trait SolverStrategyHost {
    fn set_order_status(&self, order: &mut SolverOrder, status: SolverOrderStatus);
    fn get_order_batch(&self) -> Vec<Arc<RwLock<SolverOrder>>>;
    fn get_next_batch_order_id(&self) -> BatchOrderId;
    fn get_price_tracker(&self) -> &RwLock<PriceTracker>;
    fn get_book_manager(&self) -> &RwLock<dyn OrderBookManager + Send + Sync + 'static>;
    fn get_basket_manager(&self) -> &RwLock<BasketManager>;
}

pub trait SolverStrategy {
    fn solve_engagements(
        &mut self,
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
    strategy: Arc<RwLock<dyn SolverStrategy>>,
    // dependencies
    chain_connector: Arc<RwLock<dyn ChainConnector + Send + Sync>>,
    index_order_manager: Arc<RwLock<IndexOrderManager>>,
    quote_request_manager: Arc<RwLock<dyn QuoteRequestManager + Send + Sync>>,
    basket_manager: Arc<RwLock<BasketManager>>,
    price_tracker: Arc<RwLock<PriceTracker>>,
    order_book_manager: Arc<RwLock<dyn OrderBookManager + Send + Sync>>,
    inventory_manager: Arc<RwLock<InventoryManager>>,
    order_id_provider: Arc<RwLock<dyn OrderIdProvider>>,
    // mappings
    client_funds: RwLock<HashMap<Address, Arc<RwLock<CollateralPosition>>>>,
    client_orders: RwLock<HashMap<(Address, ClientOrderId), Arc<RwLock<SolverOrder>>>>,
    engagements: RwLock<HashMap<BatchOrderId, EngagedSolverOrders>>,
    batches: RwLock<HashMap<BatchOrderId, BatchOrderStatus>>,
    // queues
    ready_funds: Mutex<VecDeque<Arc<RwLock<CollateralPosition>>>>,
    ready_orders: Mutex<VecDeque<Arc<RwLock<SolverOrder>>>>,
    ready_batches: Mutex<VecDeque<BatchOrderId>>,
    ready_mints: Mutex<VecDeque<Arc<RwLock<SolverOrder>>>>,
    // parameters
    max_batch_size: usize,
    zero_threshold: Amount,
    fill_threshold: Amount,
    mint_threshold: Amount,
    fund_wait_period: TimeDelta,
    mint_wait_period: TimeDelta,
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
        strategy: Arc<RwLock<dyn SolverStrategy>>,
        order_id_provider: Arc<RwLock<dyn OrderIdProvider>>,
        max_batch_size: usize,
        zero_threshold: Amount,
        fill_threshold: Amount,
        mint_threshold: Amount,
        fund_wait_period: TimeDelta,
        mint_wait_period: TimeDelta,
    ) -> Self {
        Self {
            // dependencies
            chain_connector,
            index_order_manager,
            quote_request_manager,
            basket_manager,
            price_tracker,
            order_book_manager,
            inventory_manager,
            strategy,
            order_id_provider,
            // mappings
            client_orders: RwLock::new(HashMap::new()),
            client_funds: RwLock::new(HashMap::new()),
            engagements: RwLock::new(HashMap::new()),
            batches: RwLock::new(HashMap::new()),
            // queues
            ready_funds: Mutex::new(VecDeque::new()),
            ready_orders: Mutex::new(VecDeque::new()),
            ready_batches: Mutex::new(VecDeque::new()),
            ready_mints: Mutex::new(VecDeque::new()),
            // parameters
            max_batch_size,
            zero_threshold,
            fill_threshold,
            mint_threshold,
            fund_wait_period,
            mint_wait_period,
        }
    }

    fn engage_more_orders(&self) -> Result<()> {
        if let Some(engaged_orders) = self.strategy.write().solve_engagements(self)? {
            let send_engage = engaged_orders
                .engaged_orders
                .iter()
                .map_while(|order| {
                    let order = order.write();
                    if order.engaged_collateral < self.zero_threshold {
                        None
                    } else {
                        Some((
                            order.address,
                            order.client_order_id.clone(),
                            order.symbol.clone(),
                            order.engaged_collateral,
                        ))
                    }
                })
                .collect_vec();

            if send_engage.len() != engaged_orders.engaged_orders.len() {
                return Err(eyre!("Zero Quantity"));
            }

            let batch_order_id = match self
                .engagements
                .write()
                .entry(engaged_orders.batch_order_id.clone())
            {
                Entry::Vacant(entry) => entry.insert(engaged_orders).batch_order_id.clone(),
                Entry::Occupied(_) => Err(eyre!("Dublicate Batch Id"))?,
            };

            self.index_order_manager
                .write()
                .engage_orders(batch_order_id, send_engage)?;
        }
        Ok(())
    }

    fn send_batch(
        &self,
        engaged_orders: &EngagedSolverOrders,
        timestamp: DateTime<Utc>,
    ) -> Result<()> {
        // TODO: we should generate IDs
        let batch_order_id = &engaged_orders.batch_order_id;

        // TODO: we should compact these batches (and do matrix solving)
        let batches = engaged_orders
            .engaged_orders
            .iter()
            .map_while(|engage_order| {
                let engage_order = engage_order.read();
                Some(Arc::new(BatchOrder {
                    batch_order_id: batch_order_id.clone(),
                    created_timestamp: timestamp,
                    asset_orders: engage_order
                        .basket
                        .basket_assets
                        .iter()
                        .map(|basket_asset| {
                            let asset_symbol = &basket_asset.weight.asset.name;
                            let price =
                                *engage_order.asset_price_limits.get(&asset_symbol).unwrap();
                            let quantity =
                                *engage_order.asset_quantities.get(&asset_symbol).unwrap();
                            AssetOrder {
                                order_id: self.order_id_provider.write().next_order_id(),
                                price,
                                quantity,
                                side: engage_order.engaged_side,
                                symbol: basket_asset.weight.asset.name.clone(),
                            }
                        })
                        .collect_vec(),
                }))
            })
            .collect_vec();

        for batch in batches {
            println!(
                "Sending Batch: {}",
                batch
                    .asset_orders
                    .iter()
                    .map(|ba| format!(
                        "{:?} {}: {:0.5} @ {:0.5}",
                        ba.side, ba.symbol, ba.quantity, ba.price
                    ))
                    .join("; ")
            );
            let mut batch_order_status =
                BatchOrderStatus::new(batch.batch_order_id.clone(), batch.created_timestamp);

            for order in &batch.asset_orders {
                batch_order_status
                    .try_add_position(order, batch.created_timestamp)
                    .ok_or_eyre("Math Problem")?;
            }

            self.batches
                .write()
                .insert(batch.batch_order_id.clone(), batch_order_status)
                .is_none()
                .then_some(())
                .ok_or_eyre("Duplicate batch ID")?;

            self.inventory_manager.write().new_order(batch)?;
        }
        Ok(())
    }

    fn send_more_batches(&self, timestamp: DateTime<Utc>) -> Result<()> {
        let engagements = self.engagements.read();

        let new_engagements = (|| {
            let mut new_batches = self.ready_batches.lock();
            let max_drain = new_batches.len().min(self.max_batch_size);
            new_batches
                .drain(..max_drain)
                .filter_map(|batch_order_id| engagements.get(&batch_order_id))
                .collect_vec()
        })();

        for engaged_orders in new_engagements {
            self.send_batch(engaged_orders, timestamp)?;
        }

        Ok(())
    }

    fn fill_index_order(
        &self,
        batch: &mut BatchOrderStatus,
        engaged_order: &RwLock<SolverOrderEngagement>,
    ) -> Result<Option<Arc<RwLock<SolverOrder>>>> {
        let mut engaged_order = engaged_order.upgradable_read();
        let engaged_quantity = engaged_order.engaged_quantity;
        let index_order = engaged_order.index_order.clone();
        let mut index_order_write = index_order.write();

        // We search for fill-rate of this Index Order matching against
        // available lots in this batch. Note that this is fill-rate in
        // this batch, and 100% means that only the fraction of the Index Order
        // that was included in this batch is fully filled, and there might
        // still be more quantity outside of this batch on this Index Order
        // that will need to be filled at later time in another batch.
        let mut fill_rate = None;

        for asset in &engaged_order.basket.basket_assets {
            // = Amount of an asset in Basket Definition
            //  * IndexOrder quantity engaged in this batch
            let asset_quantity =
                safe!(asset.quantity * engaged_quantity).ok_or_eyre("Math Problem")?;

            let asset_symbol = &asset.weight.asset.name;
            let key = (asset_symbol.clone(), index_order_write.side);
            let position = batch
                .positions
                .get(&key)
                .ok_or_eyre("Missing position for asset")?;

            let contribution_fraction = *engaged_order
                .asset_contribution_fractions
                .get(asset_symbol)
                .ok_or_eyre("Asset contribution fraction not found")?;

            // = Current available position from this Batch
            //  * Index Order contribution fraction in this batch
            let available_quantity =
                safe!(position.position * contribution_fraction).ok_or_eyre("Math Problem")?;

            // = Quantity available in this batch for this Index Order
            //  / Quantity required to fully fill the fraction of the whole Index Order requested in this batch
            let avialable_fill_rate =
                safe!(available_quantity / asset_quantity).ok_or_eyre("Math Problem")?;

            // We're finding lowest fill-rate for this Index Order across all assets, because
            // we cannot fill this Index Order more than least available asset fill-rate.
            fill_rate = fill_rate.map_or(Some(avialable_fill_rate), |x: Amount| {
                Some(x.min(avialable_fill_rate))
            });
            println!(
                "Fill Basket Asset: {:5} q={:0.5} p={:0.5} aq={:0.5} cf={:0.5} afr={:0.5}",
                asset_symbol,
                asset_quantity,
                position.position,
                available_quantity,
                contribution_fraction,
                avialable_fill_rate
            );
        }

        let fill_rate = fill_rate.expect("Fill-rate must have been known at this stage");

        // This is new filled quantity of this batch engagement with Index Order
        let filled_quantity = safe!(fill_rate * engaged_quantity).ok_or_eyre("Math Problem")?;

        // This is how much it has changed since last time it was updated
        let filled_quantity_delta =
            safe!(filled_quantity - engaged_order.filled_quantity).ok_or_eyre("Math Problem")?;

        // We didn't fill any quantity on that index order
        if filled_quantity_delta < self.zero_threshold {
            return Ok(None);
        }

        // Allocate batch lots to index order
        //
        // NOTE This is super important as we want to allocate prices and fees
        // dependant on concrete execution(s). Say one asset had multiple
        // executions at different prices, we need to allocate to Index Orders
        // some portion of those quantites at some prices, and some portion of
        // fees. When we Mint Index we need to sum those fees and lot values allocated
        // per Index Order for which we're minting.
        //
        for asset in &engaged_order.basket.basket_assets {
            let asset_quantity =
                safe!(asset.quantity * filled_quantity_delta).ok_or_eyre("Math Problem")?;

            let asset_symbol = &asset.weight.asset.name;
            let key = (asset_symbol.clone(), index_order_write.side);
            let position = batch
                .positions
                .get_mut(&key)
                .expect("Asset position must be known at this stage");

            // It is critical that this function will match exactly full
            // quantity, as otherwise our numbers wouldn't match up. That is
            // because lots are opened in total quantity matching position.
            match position.try_allocate_lots(&mut index_order_write, asset_quantity) {
                None => Err(eyre!("Math Problem")),
                Some(quantity) if quantity > self.zero_threshold => {
                    Err(eyre!("Couldn't allocate sufficient lots"))
                }
                Some(_) => Ok(()),
            }?;
        }

        // Now we update it
        engaged_order.with_upgraded(|x| {
            x.filled_quantity = filled_quantity;
        });

        // And we add the delta to the Index Order filled quantity
        index_order_write.filled_quantity =
            safe!(index_order_write.filled_quantity + filled_quantity_delta)
                .ok_or_eyre("Math Problem")?;

        index_order_write.engaged_collateral =
            safe!(index_order_write.engaged_collateral - filled_quantity_delta)
                .ok_or_eyre("Math Problem")?;

        index_order_write.timestamp = batch.last_update_timestamp;

        let remaining_quantity =
            safe!(index_order_write.remaining_collateral + index_order_write.engaged_collateral)
                .ok_or_eyre("Math Problem")?;

        let total_quantity = safe!(index_order_write.filled_quantity + remaining_quantity)
            .ok_or_eyre("Math Problem")?;

        let order_fill_rate =
            safe!(index_order_write.filled_quantity / total_quantity).ok_or_eyre("Math Problem")?;

        println!(
            "Fill Index Order: ifq={:0.5} irq={:0.5} ieq={:0.5} rq={:0.5} bfr={:0.3}% ofr={:0.3}%",
            index_order_write.filled_quantity,
            index_order_write.remaining_collateral,
            index_order_write.engaged_collateral,
            remaining_quantity,
            safe!(fill_rate * Amount::ONE_HUNDRED).ok_or_eyre("Math Problem")?,
            safe!(order_fill_rate * Amount::ONE_HUNDRED).ok_or_eyre("Math Problem")?,
        );

        let collateral_spent = Amount::ZERO;

        self.index_order_manager.write().fill_order_request(
            &index_order_write.address,
            &index_order_write.original_client_order_id,
            &index_order_write.symbol,
            collateral_spent,
            filled_quantity_delta,
            batch.last_update_timestamp,
        )?;

        match index_order_write.status {
            SolverOrderStatus::PartlyMintable => {
                // then index order is already in the mintable queue
            }
            SolverOrderStatus::FullyMintable => {
                // then index order is already in the mintable queue
            }
            _ if self.mint_threshold < order_fill_rate => {
                self.set_order_status(&mut index_order_write, SolverOrderStatus::PartlyMintable);
                self.ready_mints.lock().push_back(index_order.clone());
            }
            _ => {
                // not mintable yet
            }
        }

        if self.fill_threshold < order_fill_rate {
            self.set_order_status(&mut index_order_write, SolverOrderStatus::FullyMintable);
        } else if self.fill_threshold < fill_rate {
            return Ok(Some(index_order.clone()));
        }

        Ok(None)
    }

    fn fill_batch_orders(&self, batch: &mut BatchOrderStatus) -> Result<()> {
        let engagements_read = self.engagements.read();
        let engagement = engagements_read
            .get(&batch.batch_order_id)
            .ok_or_else(|| eyre!("Engagement not found {}", batch.batch_order_id))?;

        let mut ready_orders = VecDeque::new();

        for engaged_order in &engagement.engaged_orders {
            if let Some(index_order) = self.fill_index_order(batch, engaged_order)? {
                ready_orders.push_back(index_order);
            }
        }

        self.ready_orders.lock().extend(ready_orders.drain(..));
        Ok(())
    }

    fn mint_indexes(&self, timestamp: DateTime<Utc>) -> Result<()> {
        let ready_timestamp = timestamp - self.mint_wait_period;
        let check_not_ready = |x: &SolverOrder| ready_timestamp < x.timestamp;
        let ready_mints = (|| {
            let mut mints = self.ready_mints.lock();
            let res = mints.iter().find_position(|p| check_not_ready(&p.read()));
            if let Some((pos, _)) = res {
                mints.drain(..pos)
            } else {
                mints.drain(..)
            }
            .collect_vec()
        })();

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

        let client_funds = self.client_funds.read();
        let funds = client_funds
            .get(&index_order.address)
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

    fn process_credits(&self, timestamp: DateTime<Utc>) -> Result<()> {
        let ready_timestamp = timestamp - self.fund_wait_period;
        let check_not_ready = |x: &CollateralTransaction| ready_timestamp < x.timestamp;
        let ready_funds = (|| {
            let mut funds = self.ready_funds.lock();
            let res = funds.iter().find_position(|p| {
                p.read()
                    .transactions_cr
                    .front()
                    .map(check_not_ready)
                    .unwrap_or(true)
            });
            if let Some((pos, _)) = res {
                funds.drain(..pos)
            } else {
                funds.drain(..)
            }
            .collect_vec()
        })();

        for fund in ready_funds {
            let mut fund_write = fund.write();
            let res = fund_write
                .transactions_cr
                .iter()
                .find_position(|x| check_not_ready(x));
            let completed = if let Some((pos, _)) = res {
                fund_write.transactions_cr.drain(..pos)
            } else {
                fund_write.transactions_cr.drain(..)
            }
            .collect_vec();
            fund_write.balance = completed
                .iter()
                .try_fold(fund_write.balance, |balance, tx| safe!(balance + tx.amount))
                .ok_or_eyre("Math Problem")?;
            fund_write.last_update_timestamp = ready_timestamp;
            fund_write.completed_transactions.extend(completed);
            println!(
                "New balance for {} {:0.5}",
                fund_write.address, fund_write.balance
            );
        }

        Ok(())
    }

    /// Core thinking function
    pub fn solve(&self, timestamp: DateTime<Utc>) {
        println!("\nSolve...");

        //
        // check if there is some collateral we could use
        //
        if let Err(err) = self.process_credits(timestamp) {
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
        if let Err(err) = self.send_more_batches(timestamp) {
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
            } => {
                let collateral_position = self
                    .client_funds
                    .write()
                    .entry(address)
                    .or_insert_with(|| Arc::new(RwLock::new(CollateralPosition::new(address))))
                    .clone();

                (|| -> Result<()> {
                    let mut p = collateral_position.write();

                    p.transactions_cr.push_back(CollateralTransaction {
                        payment_id,
                        amount,
                        timestamp,
                    });
                    p.pending_cr = safe!(p.pending_cr + amount).ok_or_eyre("Math Problem")?;
                    p.last_update_timestamp = timestamp;
                    Ok(())
                })()?;

                self.ready_funds.lock().push_back(collateral_position);
                Ok(())
            }
            ChainNotification::WithdrawalRequest {
                address,
                payment_id,
                amount,
                timestamp,
            } => {
                let collateral_position = self
                    .client_funds
                    .write()
                    .entry(address)
                    .or_insert_with(|| Arc::new(RwLock::new(CollateralPosition::new(address))))
                    .clone();

                (|| -> Result<()> {
                    let mut p = collateral_position.write();

                    p.transactions_dr.push_back(CollateralTransaction {
                        payment_id,
                        amount,
                        timestamp,
                    });
                    p.pending_dr = safe!(p.pending_dr + amount).ok_or_eyre("Math Problem")?;
                    p.last_update_timestamp = timestamp;
                    Ok(())
                })()?;

                self.ready_funds.lock().push_back(collateral_position);
                Ok(())
            }
        }
    }

    /// receive Index Order
    pub fn handle_index_order(&self, notification: IndexOrderEvent) {
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
                            collateral_spent: Amount::ZERO,
                            filled_quantity: Amount::ZERO,
                            timestamp,
                            status: SolverOrderStatus::Open,
                            lots: Vec::new(),
                        }));
                        entry.insert(solver_order.clone());
                        self.ready_orders.lock().push_back(solver_order);
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
                timestamp: _,
            } => {
                println!(
                    "\nSolver: Handle Index Order EngageIndexOrder {}",
                    batch_order_id
                );
                match self.engagements.write().get_mut(&batch_order_id) {
                    Some(engaged_orders_stored) => {
                        engaged_orders_stored
                            .engaged_orders
                            .retain_mut(|engaged_order_stored| {
                                let mut engaged_order_stored = engaged_order_stored.write();
                                let index_order = engaged_order_stored.index_order.clone();
                                let mut index_order_stored = index_order.write();
                                match engaged_orders.get(&(
                                    engaged_order_stored.address,
                                    engaged_order_stored.client_order_id.clone(),
                                )) {
                                    Some(engaged_order) => {
                                        index_order_stored.remaining_collateral =
                                            engaged_order.collateral_remaining;
                                        index_order_stored.engaged_collateral =
                                            engaged_order.collateral_engaged;
                                        engaged_order_stored.engaged_quantity =
                                            engaged_order.collateral_engaged;
                                    }
                                    None => {
                                        self.set_order_status(
                                            &mut index_order_stored,
                                            SolverOrderStatus::MathOverflow,
                                        );
                                    }
                                }
                                true
                            });
                    }
                    None => {
                        todo!()
                    }
                }
                self.ready_batches.lock().push_back(batch_order_id);
            }
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
                    (|| safe!(safe!(fee * Amount::ONE_HUNDRED) / safe!(quantity * price)?))().ok_or_eyre("Math Problem")?
                );
                let mut write_batches = self.batches.write();
                let batch = write_batches
                    .get_mut(&batch_order_id)
                    .ok_or_eyre("Missing Batch")?;

                let key = (symbol.clone(), side);
                let position = batch
                    .positions
                    .get_mut(&key)
                    .ok_or_eyre("Missing Position")?;

                (|| {
                    position.position = safe!(position.position + quantity)?;

                    let fraction_delta = safe!(quantity / position.order_quantity)?;
                    let filled_asset_volley = safe!(fraction_delta * position.volley_size)?;
                    batch.filled_volley = safe!(batch.filled_volley + filled_asset_volley)?;
                    batch.filled_fraction = safe!(batch.filled_volley / batch.volley_size)?;

                    let filled_value = safe!(quantity * price)?;
                    position.realized_value = safe!(position.realized_value + filled_value)?;
                    batch.realized_value = safe!(batch.realized_value + filled_value)?;

                    position.fee = safe!(position.fee + fee)?;
                    batch.fee = safe!(batch.fee + fee)?;

                    position.open_lots.push_back(BatchAssetLot {
                        order_id,
                        lot_id,
                        original_quantity: quantity,
                        remaining_quantity: quantity,
                        price,
                        fee,
                        timestamp,
                    });

                    position.last_update_timestamp = timestamp;

                    println!(
                        "Batch Position: {:?} {:5} price={:0.5} volley={:0.5} real={:0.5} + fee={:0.5}",
                        position.side,
                        position.symbol,
                        position.order_quantity,
                        position.volley_size,
                        position.realized_value,
                        position.fee
                    );

                    println!(
                        "Batch Status: {} volley={:0.5} fill={:0.5} frac={:0.5} real={:0.5} fee={:0.5}",
                        batch_order_id,
                        batch.volley_size,
                        batch.filled_volley,
                        batch.filled_fraction,
                        batch.realized_value,
                        batch.fee);
                    Some(())
                })()
                .ok_or_eyre("Math Problem")?;

                batch.last_update_timestamp = timestamp;

                self.fill_batch_orders(batch)
            }
            InventoryEvent::CloseLot {
                original_order_id: _,
                original_batch_order_id: _,
                original_lot_id: _,
                closing_order_id: _,
                closing_batch_order_id: _,
                closing_lot_id: _,
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
                closing_timestamp: _,
            } => {
                println!(
                    "\nSolver: Handle Inventory Event CloseLot {:?} {:5} {:0.5}@{:0.5}+{:0.5} ({:0.5}%)",
                    side,
                    symbol,
                    quantity_closed,
                    closing_price,
                    closing_fee,
                    Amount::ONE_HUNDRED * (original_quantity - quantity_remaining)
                        / original_quantity
                );
                Ok(())
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

impl SolverStrategyHost for Solver {
    fn get_order_batch(&self) -> Vec<Arc<RwLock<SolverOrder>>> {
        (|| {
            let mut new_orders = self.ready_orders.lock();
            let max_drain = new_orders.len().min(self.max_batch_size);
            new_orders.drain(..max_drain).collect_vec()
        })()
    }

    fn get_next_batch_order_id(&self) -> BatchOrderId {
        self.order_id_provider.write().next_batch_order_id()
    }

    fn get_price_tracker(&self) -> &RwLock<PriceTracker> {
        &self.price_tracker
    }

    fn get_book_manager(&self) -> &RwLock<dyn OrderBookManager + Send + Sync + 'static> {
        &self.order_book_manager
    }

    fn get_basket_manager(&self) -> &RwLock<BasketManager> {
        &self.basket_manager
    }

    fn set_order_status(&self, order: &mut SolverOrder, status: SolverOrderStatus) {
        println!(
            "Set Index Order Status: {} {:?}",
            order.client_order_id, status
        );
        order.status = status;
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

        let solver_strategy = Arc::new(RwLock::new(SimpleSolver::new(
            dec!(0.01),
            dec!(1.001),
            dec!(1500.0),
            dec!(1200.0),
        )));

        let solver = Arc::new(Solver::new(
            chain_connector.clone(),
            index_order_manager.clone(),
            quote_request_manager.clone(),
            basket_manager.clone(),
            price_tracker.clone(),
            order_book_manager.clone(),
            inventory_manager.clone(),
            solver_strategy.clone(),
            order_id_provider.clone(),
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
                    recv(index_order_receiver) -> res => solver.handle_index_order(res.unwrap()),
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

        let collateral_amount = dec!(1005.0) * dec!(2.5) * (Amount::ONE + dec!(0.001));

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
        assert_decimal_approx_eq!(order1.quantity, dec!(19.9942), tolerance);
        assert_decimal_approx_eq!(order2.price, dec!(303.00), tolerance);
        assert_decimal_approx_eq!(order2.quantity, dec!(1.6273), tolerance);

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
