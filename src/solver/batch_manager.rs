use std::{
    collections::{hash_map::Entry, HashMap, VecDeque},
    sync::Arc,
};

use chrono::{DateTime, TimeDelta, Utc};
use eyre::{eyre, OptionExt, Result};
use itertools::Itertools;
use parking_lot::{Mutex, RwLock};
use safe_math::safe;

use crate::solver::solver_order::SolverOrderStatus;
use symm_core::{
    core::{
        bits::{
            Address, Amount, AssetOrder, BatchOrder, BatchOrderId, ClientOrderId, OrderId, Side,
            Symbol,
        },
        decimal_ext::DecimalExt,
        functional::{IntoObservableSingle, PublishSingle, SingleObserver},
    },
    order_sender::position::LotId,
};

use super::{
    index_order_manager::EngagedIndexOrder,
    solver::{EngagedSolverOrders, SetSolverOrderStatus, SolverOrderEngagement},
    solver_order::{SolverOrder, SolverOrderAssetLot},
};

pub enum BatchEvent {
    BatchComplete {
        batch_order_id: BatchOrderId,
        continued_orders: Vec<Arc<RwLock<SolverOrder>>>,
    },
    BatchMintable {
        mintable_orders: Vec<Arc<RwLock<SolverOrder>>>,
    },
}

/// Every execution of any order in the batch produces a lot of the asset on the
/// order.
///
/// Note that InventoryManager also manages lots, however these are lots in our
/// inventory, for us to know what we've got in the inventory and how we
/// acquired it. The BatchAssetLot is different kind of lot, in which we store
/// one unit of execution for a batch created by Solver, and then we allocate
/// some portion of that unit to index orders, using allocations list.
pub struct BatchAssetLot {
    /// Asset Order ID
    pub order_id: OrderId,

    /// Transaction ID
    pub lot_id: LotId,

    /// Closing Transaction ID
    pub closing_lot_id: Option<LotId>,

    /// Quantity filled
    pub original_quantity: Amount,

    /// Unallocated quantity
    pub remaining_quantity: Amount,

    /// Executed price
    pub price: Amount,

    /// Execution fee
    pub fee: Amount,

    /// Timestamp of execution
    pub timestamp: DateTime<Utc>,
}

impl BatchAssetLot {
    fn build_solver_asset_lot(
        &self,
        symbol: Symbol,
        quantity: Amount,
    ) -> Option<SolverOrderAssetLot> {
        Some(SolverOrderAssetLot {
            lot_id: self.lot_id.clone(),
            symbol,
            quantity,
            price: self.price,
            fee: safe!(self.fee * safe!(quantity / self.original_quantity)?)?,
        })
    }

    fn split(&mut self) -> BatchAssetLot {
        let carried_quantity = self.remaining_quantity;
        self.remaining_quantity = Amount::ZERO;
        BatchAssetLot {
            order_id: self.order_id.clone(),
            lot_id: self.lot_id.clone(),
            closing_lot_id: self.closing_lot_id.clone(),
            remaining_quantity: carried_quantity,
            ..*self
        }
    }
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

    /// Tell whether we should expect more updates to this position or not
    is_cancelled: bool,

    /// Tell if there was any quantity that was cancelled
    quantity_cancelled: Amount,

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
            is_cancelled: false,
            quantity_cancelled: Amount::ZERO,
            last_update_timestamp: timestamp,
            open_lots: VecDeque::new(),
            closed_lots: VecDeque::new(),
        })
    }

    pub fn try_allocate_lots(
        &mut self,
        index_order: &mut SolverOrder,
        mut quantity: Amount,
        tolerance: Amount,
    ) -> Option<(Amount, Amount)> {
        let mut collateral_spent = Amount::ZERO;
        let mut push_allocation = |lot: &mut BatchAssetLot, quantity: Amount| -> Option<()> {
            let asset_allocation = lot.build_solver_asset_lot(self.symbol.clone(), quantity)?;
            let lot_collateral_spent = asset_allocation.compute_collateral_spent()?;
            collateral_spent = safe!(collateral_spent + lot_collateral_spent)?;
            tracing::info!(
                "(batch-asset-position) IndexOrder allocation for {} {} {} q={:0.5} p={:0.5} fee={:0.5}",
                index_order.client_order_id,
                asset_allocation.lot_id,
                asset_allocation.symbol,
                asset_allocation.quantity,
                asset_allocation.price,
                asset_allocation.fee
            );
            index_order.lots.push(asset_allocation);
            Some(())
        };
        while let Some(lot) = self.open_lots.front_mut() {
            let remaining_quantity = safe!(quantity - lot.remaining_quantity)?;
            if -tolerance < remaining_quantity {
                let mut lot = self
                    .open_lots
                    .pop_front()
                    .expect("Should have front at this stage");
                let used_quantity = lot.remaining_quantity;
                push_allocation(&mut lot, used_quantity)?;
                quantity = remaining_quantity;
                lot.remaining_quantity = Amount::ZERO;
                self.closed_lots.push_back(lot);
            } else {
                push_allocation(lot, quantity)?;
                lot.remaining_quantity = -remaining_quantity;
                quantity = Amount::ZERO;
                break;
            }
        }
        Some((quantity, collateral_spent))
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

    /// Tell is we should expect more updated from this batch
    is_cancelled: bool,

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
            is_cancelled: false,
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

    pub fn update(
        &mut self,
        order_id: OrderId,
        batch_order_id: BatchOrderId,
        lot_id: LotId,
        closing_lot_id: Option<LotId>,
        symbol: Symbol,
        side: Side,
        price: Amount,
        quantity: Amount,
        fee: Amount,
        timestamp: DateTime<Utc>,
    ) -> Result<()> {
        let key = (symbol.clone(), side);
        let position = self
            .positions
            .get_mut(&key)
            .ok_or_eyre("Cannot find position")?;

        (|| {
            position.position = safe!(position.position + quantity)?;

            let fraction_delta = safe!(quantity / position.order_quantity)?;
            let filled_asset_volley = safe!(fraction_delta * position.volley_size)?;
            self.filled_volley = safe!(self.filled_volley + filled_asset_volley)?;
            self.filled_fraction = safe!(self.filled_volley / self.volley_size)?;

            let filled_value = safe!(quantity * price)?;
            position.realized_value = safe!(position.realized_value + filled_value)?;
            self.realized_value = safe!(self.realized_value + filled_value)?;

            position.fee = safe!(position.fee + fee)?;
            self.fee = safe!(self.fee + fee)?;

            position.open_lots.push_back(BatchAssetLot {
                order_id,
                lot_id,
                closing_lot_id,
                original_quantity: quantity,
                remaining_quantity: quantity,
                price,
                fee,
                timestamp,
            });

            position.last_update_timestamp = timestamp;
            self.last_update_timestamp = timestamp;

            Some(())
        })()
        .ok_or_eyre("Math Problem")?;

        tracing::info!(
            "(batch-manager) Batch Position: {:?} {:5} total={:0.5} volley={:0.5} pos={:0.5} real={:0.5} + fee={:0.5}",
            position.side,
            position.symbol,
            position.order_quantity,
            position.volley_size,
            position.position,
            position.realized_value,
            position.fee
        );

        tracing::info!(
            "(batch-manager) Batch Status: {} volley={:0.5} fill={:0.5} frac={:0.5} real={:0.5} fee={:0.5}",
            batch_order_id,
            self.volley_size,
            self.filled_volley,
            self.filled_fraction,
            self.realized_value,
            self.fee
        );

        Ok(())
    }

    pub fn cancel(
        &mut self,
        symbol: Symbol,
        side: Side,
        quantity_cancelled: Amount,
        is_cancelled: bool,
        cancel_timestamp: DateTime<Utc>,
    ) -> Result<()> {
        let key = (symbol, side);
        let position = self
            .positions
            .get_mut(&key)
            .ok_or_eyre("Missing Position")?;

        position.is_cancelled = is_cancelled;
        position.quantity_cancelled = quantity_cancelled;
        position.last_update_timestamp = cancel_timestamp;

        if self
            .positions
            .values()
            .all(|position| position.is_cancelled)
        {
            tracing::info!(
                "(batch-manager) Batch is all cancelled {}",
                self.batch_order_id
            );
            self.is_cancelled = true;
        }

        Ok(())
    }

    pub fn carry_over(
        &mut self,
        carry_overs: &mut HashMap<(Symbol, Side), BatchCarryOver>,
    ) -> Result<()> {
        for ((symbol, side), position) in self.positions.iter_mut() {
            (|| {
                if let Some(first) = position.open_lots.front_mut() {
                    let mut carried_lots = VecDeque::new();
                    carried_lots.push_back(first.split());
                    carried_lots.extend(position.open_lots.drain(1..));
                    let carried_position =
                        carried_lots.iter().map(|lot| lot.remaining_quantity).sum();
                    tracing::info!(
                        "(batch-order-status) Carried over {} {:?} {:0.5}",
                        symbol,
                        side,
                        carried_position
                    );

                    match carry_overs.entry((symbol.clone(), *side)) {
                        Entry::Vacant(entry) => {
                            entry.insert(BatchCarryOver {
                                carried_position,
                                carried_lots,
                            });
                        }
                        Entry::Occupied(mut entry) => {
                            let data = entry.get_mut();
                            data.carried_position =
                                safe!(data.carried_position + carried_position)?;
                            data.carried_lots.extend(carried_lots);
                        }
                    }
                }

                Some(())
            })()
            .ok_or_eyre("Failed to compute carried over position")?;
        }
        Ok(())
    }

    pub fn carry_in(
        &mut self,
        carry_overs: &mut HashMap<(Symbol, Side), BatchCarryOver>,
    ) -> Result<HashMap<(Symbol, Side), Amount>> {
        let mut result = HashMap::new();
        for ((symbol, side), position) in self.positions.iter_mut() {
            match carry_overs.entry((symbol.clone(), *side)) {
                Entry::Vacant(_) => {}
                Entry::Occupied(entry) => {
                    let carried = entry.remove(); // we should only carry once
                    position.open_lots.extend(carried.carried_lots);
                    position.position = safe!(position.position + carried.carried_position)
                        .ok_or_eyre("Math Problem")?;
                    result.insert((symbol.clone(), *side), carried.carried_position);
                }
            }
        }
        Ok(result)
    }
}

struct BatchCarryOver {
    /// Total quantity carried over
    carried_position: Amount,

    /// Unallocated lots
    carried_lots: VecDeque<BatchAssetLot>,
}

pub trait BatchManagerHost: SetSolverOrderStatus {
    fn get_next_order_id(&self) -> OrderId;
    fn send_order_batch(&self, batch_order: Arc<BatchOrder>) -> Result<()>;
    fn fill_order_request(
        &self,
        chain_id: u32,
        address: &Address,
        client_order_id: &ClientOrderId,
        symbol: &Symbol,
        collateral_spent: Amount,
        fill_amount: Amount,
        timestamp: DateTime<Utc>,
    ) -> Result<()>;
}

pub struct BatchManager {
    observer: SingleObserver<BatchEvent>,
    batches: HashMap<BatchOrderId, Arc<RwLock<BatchOrderStatus>>>,
    engagements: HashMap<BatchOrderId, Arc<RwLock<EngagedSolverOrders>>>,
    ready_batches: VecDeque<BatchOrderId>,
    carry_overs: Mutex<HashMap<(Symbol, Side), BatchCarryOver>>,
    ready_mints: Mutex<VecDeque<Arc<RwLock<SolverOrder>>>>,
    total_volley_size: RwLock<Amount>,
    max_batch_size: usize,
    zero_threshold: Amount,
    fill_threshold: Amount,
    mint_threshold: Amount,
    mint_wait_period: TimeDelta,
}

impl BatchManager {
    pub fn new(
        max_batch_size: usize,
        zero_threshold: Amount,
        fill_threshold: Amount,
        mint_threshold: Amount,
        mint_wait_period: TimeDelta,
    ) -> Self {
        Self {
            observer: SingleObserver::new(),
            batches: HashMap::new(),
            engagements: HashMap::new(),
            ready_batches: VecDeque::new(),
            carry_overs: Mutex::new(HashMap::new()),
            ready_mints: Mutex::new(VecDeque::new()),
            total_volley_size: RwLock::new(Amount::ZERO),
            max_batch_size,
            zero_threshold,
            fill_threshold,
            mint_threshold,
            mint_wait_period,
        }
    }

    fn build_batch_order(
        &self,
        host: &dyn BatchManagerHost,
        engaged_orders: &EngagedSolverOrders,
        timestamp: DateTime<Utc>,
    ) -> Result<BatchOrder> {
        let batch_order_id = &engaged_orders.batch_order_id;
        let mut batch_data = HashMap::new();

        for (asset_symbol, quantity) in &engaged_orders.engaged_buys.asset_quantities {
            let price = *engaged_orders
                .engaged_buys
                .asset_price_limits
                .get(&asset_symbol)
                .ok_or_else(|| eyre!("Cannot find expected price for asset {}", asset_symbol))?;

            batch_data
                .insert(
                    asset_symbol.clone(),
                    AssetOrder {
                        order_id: host.get_next_order_id(),
                        price,
                        quantity: *quantity,
                        side: Side::Buy,
                        symbol: asset_symbol.clone(),
                    },
                )
                .is_none()
                .then_some(())
                .ok_or_else(|| eyre!("Cannot insert asset order for {}", asset_symbol))?;
        }

        let batch = BatchOrder {
            batch_order_id: batch_order_id.clone(),
            created_timestamp: timestamp,
            asset_orders: batch_data.into_values().collect_vec(),
        };

        Ok(batch)
    }

    fn adjust_order_batch(
        &self,
        batch: &mut BatchOrder,
        carried_positions: HashMap<(Symbol, Side), Amount>,
    ) -> Result<()> {
        for asset_order in batch.asset_orders.iter_mut() {
            if let Some(&carried_quantity) =
                carried_positions.get(&(asset_order.symbol.clone(), asset_order.side))
            {
                let remainng_quantity =
                    safe!(asset_order.quantity - carried_quantity).ok_or_eyre("Math Problem")?;

                if self.zero_threshold < remainng_quantity {
                    asset_order.quantity = remainng_quantity;
                } else {
                    asset_order.quantity = Amount::ZERO;
                }
            }
        }
        batch
            .asset_orders
            .retain(|asset_order| self.zero_threshold < asset_order.quantity);
        Ok(())
    }

    fn send_batch(
        &mut self,
        host: &dyn BatchManagerHost,
        engaged_orders: &EngagedSolverOrders,
        timestamp: DateTime<Utc>,
    ) -> Result<()> {
        let mut batch = self.build_batch_order(host, engaged_orders, timestamp)?;
        let mut batch_order_status =
            BatchOrderStatus::new(batch.batch_order_id.clone(), batch.created_timestamp);

        for order in &batch.asset_orders {
            batch_order_status
                .try_add_position(order, batch.created_timestamp)
                .ok_or_eyre("Math Problem")?;
        }

        let carried_positions = batch_order_status.carry_in(&mut self.carry_overs.lock())?;
        self.adjust_order_batch(&mut batch, carried_positions)?;

        tracing::info!(
            "(batch-manager) Sending Batch: {}",
            batch
                .asset_orders
                .iter()
                .map(|ba| format!(
                    "{:?} {}: {:0.5} @ {:0.5}",
                    ba.side, ba.symbol, ba.quantity, ba.price
                ))
                .join("; ")
        );

        *self.total_volley_size.write() += batch_order_status.volley_size;

        self.batches
            .insert(
                batch.batch_order_id.clone(),
                Arc::new(RwLock::new(batch_order_status)),
            )
            .is_none()
            .then_some(())
            .ok_or_eyre("Duplicate batch ID")?;

        host.send_order_batch(Arc::new(batch))?;
        Ok(())
    }

    fn fill_index_order(
        &self,
        host: &dyn BatchManagerHost,
        batch: &mut BatchOrderStatus,
        engaged_order: &mut SolverOrderEngagement,
    ) -> Result<()> {
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

            let asset_symbol = &asset.weight.asset.ticker;
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
            tracing::info!(
                "(batch-manager) Fill Basket Asset: {:5} q={:0.5} pos={:0.5} aq={:0.5} cf={:0.5} afr={:0.5}",
                asset_symbol,
                asset_quantity,
                position.position,
                available_quantity,
                contribution_fraction,
                avialable_fill_rate
            );
        }

        // The index-order fill-rate must never exceed 100%
        let fill_rate = fill_rate
            .expect("Fill-rate must have been known at this stage")
            .min(Amount::ONE);

        // This is new filled quantity of this batch engagement with Index Order
        let filled_quantity = safe!(fill_rate * engaged_quantity).ok_or_eyre("Math Problem")?;

        // This is how much it has changed since last time it was updated
        let filled_quantity_delta =
            safe!(filled_quantity - engaged_order.filled_quantity).ok_or_eyre("Math Problem")?;

        // We didn't fill any quantity on that index order
        if filled_quantity_delta < self.zero_threshold {
            return Ok(());
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
        let mut collateral_spent = Amount::ZERO;
        for asset in &engaged_order.basket.basket_assets {
            let asset_quantity =
                safe!(asset.quantity * filled_quantity_delta).ok_or_eyre("Math Problem")?;

            let asset_symbol = &asset.weight.asset.ticker;
            let key = (asset_symbol.clone(), index_order_write.side);
            let position = batch
                .positions
                .get_mut(&key)
                .expect("Asset position must be known at this stage");

            // It is critical that this function will match exactly full
            // quantity, as otherwise our numbers wouldn't match up. That is
            // because lots are opened in total quantity matching position.
            let asset_collateral_spent = match position.try_allocate_lots(
                &mut index_order_write,
                asset_quantity,
                self.zero_threshold,
            ) {
                None => Err(eyre!("Math Problem")),
                Some((quantity, collateral_spent)) if quantity > self.zero_threshold => Err(eyre!(
                    "Couldn't allocate sufficient lots for Index Order: q={:0.5} cs={:0.5} +{:0.5}",
                    quantity,
                    collateral_spent,
                    filled_quantity_delta
                )),
                Some((_, collateral_spent)) => Ok(collateral_spent),
            }?;

            // Total amount of collateral spent in this index order fill is a
            // sum of all spent across all assets that were filled in this
            // filled delta update. Note that even though we only receive a fill
            // for a single asset at a time, we would hold off filling the index
            // order(s) until other assets have some fills, and that is because
            // we cannot fill index order from individual assets, and we have to
            // already have received fills for a portfolio of assets, so that
            // together they represent portion of the index order proportional
            // to asset quantites in basket definition.
            collateral_spent =
                safe!(collateral_spent + asset_collateral_spent).ok_or_eyre("Math Problem")?
        }

        // Now we update it
        engaged_order.filled_quantity = filled_quantity;

        // And we add the delta to the Index Order filled quantity
        index_order_write.filled_quantity =
            safe!(index_order_write.filled_quantity + filled_quantity_delta)
                .ok_or_eyre("Math Problem")?;

        index_order_write.engaged_collateral =
            safe!(index_order_write.engaged_collateral - collateral_spent)
                .ok_or_eyre("Math Problem")?;

        index_order_write.collateral_spent =
            safe!(index_order_write.collateral_spent + collateral_spent)
                .ok_or_eyre("Math Problem")?;

        index_order_write.timestamp = batch.last_update_timestamp;

        let remaining_collateral =
            safe!(index_order_write.remaining_collateral + index_order_write.engaged_collateral)
                .ok_or_eyre("Math Problem")?;

        let total_collateral = safe!(index_order_write.collateral_spent + remaining_collateral)
            .ok_or_eyre("Math Problem")?;

        let order_fill_rate = safe!(index_order_write.collateral_spent / total_collateral)
            .ok_or_eyre("Math Problem")?;

        tracing::info!(
            "(batch-manager) Fill Index Order: ifq={:0.5} irc={:0.5} iec={:0.5} ics={:0.5} cs={:0.5} rc={:0.5} bfr={:0.3}% ofr={:0.3}%",
            index_order_write.filled_quantity,
            index_order_write.remaining_collateral,
            index_order_write.engaged_collateral,
            index_order_write.collateral_spent,
            collateral_spent,
            remaining_collateral,
            safe!(fill_rate * Amount::ONE_HUNDRED).unwrap_or_default(),
            safe!(order_fill_rate * Amount::ONE_HUNDRED).unwrap_or_default(),
        );

        host.fill_order_request(
            index_order_write.chain_id,
            &index_order_write.address,
            &index_order_write.client_order_id,
            &index_order_write.symbol,
            collateral_spent,
            filled_quantity_delta,
            batch.last_update_timestamp,
        )?;

        match index_order_write.status {
            SolverOrderStatus::Engaged if self.mint_threshold < order_fill_rate => {
                host.set_order_status(&mut index_order_write, SolverOrderStatus::PartlyMintable);
                self.ready_mints.lock().push_back(index_order.clone());
            }
            _ => {
                // not mintable yet
            }
        }

        if self.fill_threshold < order_fill_rate {
            tracing::info!(
                "(batch-manager) Index Order {} fill-rate {:0.5} is above fill threshold {:0.5}",
                index_order_write.client_order_id,
                order_fill_rate,
                self.fill_threshold
            );
            host.set_order_status(&mut index_order_write, SolverOrderStatus::FullyMintable);
        }

        Ok(())
    }

    fn fill_batch_orders(
        &self,
        host: &dyn BatchManagerHost,
        batch: Arc<RwLock<BatchOrderStatus>>,
    ) -> Result<()> {
        let batch_order_id = batch.read().batch_order_id.clone();
        let engagement = self
            .engagements
            .get(&batch_order_id)
            .ok_or_else(|| eyre!("Engagement not found {}", batch_order_id))?;

        for engaged_order in engagement.write().engaged_buys.engaged_orders.iter_mut() {
            self.fill_index_order(host, &mut batch.write(), engaged_order)?
        }

        Ok(())
    }

    pub fn cleanup_batches(&mut self) -> Result<()> {
        self.batches.retain(|key, batch| {
            if batch.read().is_cancelled {
                let _ = self.engagements.remove(key);
                false
            } else {
                true
            }
        });
        Ok(())
    }

    pub fn send_more_batches(
        &mut self,
        host: &dyn BatchManagerHost,
        timestamp: DateTime<Utc>,
    ) -> Result<()> {
        let new_engagements = {
            let new_batches = &mut self.ready_batches;
            let max_drain = new_batches.len().min(self.max_batch_size);
            new_batches
                .drain(..max_drain)
                .filter_map(|batch_order_id| self.engagements.get(&batch_order_id))
                .cloned()
                .collect_vec()
        };

        for engaged_orders in new_engagements {
            self.send_batch(host, &engaged_orders.read(), timestamp)?;
        }

        Ok(())
    }

    pub fn process_batches(
        &mut self,
        host: &dyn BatchManagerHost,
        timestamp: DateTime<Utc>,
    ) -> Result<()> {
        self.send_more_batches(host, timestamp)?;
        self.cleanup_batches()?;

        let mintable_orders = self.get_mintable_batch(timestamp);
        if !mintable_orders.is_empty() {
            self.observer
                .publish_single(BatchEvent::BatchMintable { mintable_orders });
        }

        Ok(())
    }

    pub fn get_total_volley_size(&self) -> Amount {
        *self.total_volley_size.read()
    }

    fn get_mintable_batch(&self, timestamp: DateTime<Utc>) -> Vec<Arc<RwLock<SolverOrder>>> {
        let ready_timestamp = timestamp - self.mint_wait_period;
        let check_not_ready = |x: &SolverOrder| ready_timestamp < x.timestamp;
        let ready_mints = {
            let mut mints = self.ready_mints.lock();
            let res = mints.iter().find_position(|p| check_not_ready(&p.read()));
            if let Some((pos, _)) = res {
                mints.drain(..pos)
            } else {
                mints.drain(..)
            }
            .collect_vec()
        };
        ready_mints
    }

    pub fn handle_new_engagement(
        &mut self,
        engaged_orders: Arc<RwLock<EngagedSolverOrders>>,
    ) -> Result<BatchOrderId> {
        let batch_order_id = engaged_orders.read().batch_order_id.clone();
        match self.engagements.entry(batch_order_id.clone()) {
            Entry::Vacant(entry) => entry.insert(engaged_orders),
            Entry::Occupied(_) => Err(eyre!("Dublicate Batch Id: {}", batch_order_id))?,
        };
        Ok(batch_order_id)
    }

    pub fn handle_engage_index_order(
        &mut self,
        host: &dyn BatchManagerHost,
        batch_order_id: BatchOrderId,
        engaged_orders: HashMap<(Address, ClientOrderId), EngagedIndexOrder>,
        timestamp: DateTime<Utc>,
    ) -> Result<()> {
        tracing::info!(
            "(batch-manager) Handle Index Order EngageIndexOrder {}",
            batch_order_id
        );
        match self.engagements.get(&batch_order_id) {
            Some(engaged_orders_stored) => {
                engaged_orders_stored
                    .write()
                    .engaged_buys
                    .engaged_orders
                    .iter_mut()
                    .for_each(|engaged_order_stored| {
                        let index_order = engaged_order_stored.index_order.clone();
                        let mut index_order_stored = index_order.write();

                        match engaged_orders.get(&(
                            engaged_order_stored.address,
                            engaged_order_stored.client_order_id.clone(),
                        )) {
                            Some(engaged_order) => {
                                match safe!(
                                    engaged_order.collateral_engaged
                                        + index_order_stored.collateral_carried
                                ) {
                                    Some(engaged_collateral) => {
                                        index_order_stored.engaged_collateral = engaged_collateral;
                                        engaged_order_stored.engaged_collateral =
                                            engaged_collateral;
                                    }
                                    None => {
                                        host.set_order_status(
                                            &mut index_order_stored,
                                            SolverOrderStatus::MathOverflow,
                                        );
                                        return;
                                    }
                                };

                                index_order_stored.remaining_collateral =
                                    engaged_order.collateral_remaining;

                                index_order_stored.collateral_carried = Amount::ZERO;

                                index_order_stored.timestamp = timestamp;

                                if let SolverOrderStatus::Ready = index_order_stored.status {
                                    host.set_order_status(
                                        &mut index_order_stored,
                                        SolverOrderStatus::Engaged,
                                    );
                                }
                            }
                            None => {
                                host.set_order_status(
                                    &mut index_order_stored,
                                    SolverOrderStatus::InvalidOrder,
                                );
                            }
                        }
                    });
                self.ready_batches.push_back(batch_order_id);
            }
            None => {
                todo!()
            }
        }
        Ok(())
    }

    pub fn handle_new_lot(
        &self,
        host: &dyn BatchManagerHost,
        order_id: OrderId,
        batch_order_id: BatchOrderId,
        lot_id: LotId,
        closing_lot_id: Option<LotId>,
        symbol: Symbol,
        side: Side,
        price: Amount,
        quantity: Amount,
        fee: Amount,
        timestamp: DateTime<Utc>,
    ) -> Result<()> {
        let batch = self
            .batches
            .get(&batch_order_id)
            .cloned()
            .ok_or_eyre("Missing Batch")?;

        batch.write().update(
            order_id,
            batch_order_id,
            lot_id,
            closing_lot_id,
            symbol,
            side,
            price,
            quantity,
            fee,
            timestamp,
        )?;

        self.fill_batch_orders(host, batch)
    }

    pub fn handle_cancel_order(
        &self,
        host: &dyn BatchManagerHost,
        batch_order_id: BatchOrderId,
        symbol: Symbol,
        side: Side,
        quantity_cancelled: Amount,
        is_cancelled: bool,
        cancel_timestamp: DateTime<Utc>,
    ) -> Result<()> {
        let _ = host;
        let batch = self
            .batches
            .get(&batch_order_id)
            .cloned()
            .ok_or_eyre("Missing Batch")?;

        batch.write().cancel(
            symbol,
            side,
            quantity_cancelled,
            is_cancelled,
            cancel_timestamp,
        )?;

        let is_all_cancelled = batch.read().is_cancelled;
        if is_all_cancelled {
            //
            // TODO: Inventory Manager will let us know if batch is complete, and
            // we should use that information to:
            //  - carry-over any position that we didn't use to fill an index
            //  order into next batch.
            //  - remove batch status to save memory
            //  - also need to carry over the index orders
            //
            let mut carry_overs = self.carry_overs.lock();
            batch.write().carry_over(&mut carry_overs)?;

            let engagement = self
                .engagements
                .get(&batch_order_id)
                .cloned()
                .ok_or_eyre("Missing engagement")?;

            let mut continued_orders = Vec::new();

            for engaged_order in engagement.read().engaged_buys.engaged_orders.iter() {
                let mut index_order = engaged_order.index_order.upgradable_read();
                let collateral_carried = index_order.engaged_collateral;

                if self.zero_threshold < collateral_carried {
                    if let SolverOrderStatus::FullyMintable = index_order.status {
                        tracing::info!(
                            "(batch-manager) Index Order is Fully Mintable {} cc={:0.5}",
                            engaged_order.client_order_id,
                            collateral_carried
                        );
                    } else {
                        tracing::info!(
                            "(batch-manager) Will continue Index Order {} cc={:0.5}",
                            engaged_order.client_order_id,
                            collateral_carried
                        );
                        index_order.with_upgraded(|index_order_write| {
                            index_order_write.collateral_carried = collateral_carried;
                            index_order_write.engaged_collateral = Amount::ZERO;
                        });
                        continued_orders.push(engaged_order.index_order.clone());
                    }
                }
            }

            *self.total_volley_size.write() -= batch.read().volley_size;

            self.observer.publish_single(BatchEvent::BatchComplete {
                batch_order_id,
                continued_orders,
            });
        }

        Ok(())
    }
}

impl IntoObservableSingle<BatchEvent> for BatchManager {
    fn get_single_observer_mut(&mut self) -> &mut SingleObserver<BatchEvent> {
        &mut self.observer
    }
}

#[cfg(test)]
mod test {
    use std::{
        collections::{HashMap, VecDeque},
        sync::Arc,
    };

    use chrono::{TimeDelta, Utc};
    use eyre::Result;
    use itertools::Itertools;
    use parking_lot::RwLock;
    use rust_decimal::dec;
    use test_case::test_case;

    use symm_core::{
        assert_decimal_approx_eq,
        core::{
            bits::{
                Address, Amount, AssetOrder, BatchOrder, ClientOrderId, OrderId, PaymentId, Side,
                Symbol,
            },
            functional::IntoObservableSingle,
            test_util::{
                flag_mock_atomic_bool, get_mock_address_1, get_mock_asset_1_arc,
                get_mock_asset_name_1, get_mock_atomic_bool_pair, test_mock_atomic_bool,
            },
        },
    };

    use crate::{
        index::basket::{AssetWeight, Basket, BasketDefinition},
        solver::{
            batch_manager::BatchEvent,
            index_order_manager::EngagedIndexOrder,
            solver::{
                EngagedSolverOrders, EngagedSolverOrdersSide, SetSolverOrderStatus,
                SolverOrderEngagement,
            },
            solver_order::{SolverOrder, SolverOrderAssetLot, SolverOrderStatus},
            solver_quote::{SolverQuote, SolverQuoteStatus},
        },
    };

    use super::{BatchAssetLot, BatchAssetPosition, BatchManager, BatchManagerHost};

    #[test_case(
        "C-01".into(),
        "P-01".into(),
        "O-01".into(),
        dec!(0.0),
        dec!(2000.0),
        dec!(1200.0),
        dec!(120.0),
        dec!(10.0),
        vec![],
        vec![],
        vec![],
        vec![],
        dec!(0.0); "no fills, no lots"
    )]
    #[test_case(
        "C-01".into(),
        "P-01".into(),
        "O-01".into(),
        dec!(5.0),
        dec!(2000.0),
        dec!(1200.0),
        dec!(120.0),
        dec!(10.0),
        vec![("L-01", dec!(100.0), dec!(8.0), dec!(0.8))],
        vec![("L-01", dec!(100.0), dec!(8.0), dec!(0.8), dec!(3.0))],
        vec![],
        vec![("L-01", dec!(100.0), dec!(5.0), dec!(0.5))],
        dec!(500.5); "single lot, partly closed"
    )]
    #[test_case(
        "C-01".into(),
        "P-01".into(),
        "O-01".into(),
        dec!(5.0),
        dec!(2000.0),
        dec!(1200.0),
        dec!(120.0),
        dec!(10.0),
        vec![("L-01", dec!(100.0), dec!(5.0), dec!(0.5))],
        vec![],
        vec![("L-01", dec!(100.0), dec!(5.0), dec!(0.5), dec!(0.0))],
        vec![("L-01", dec!(100.0), dec!(5.0), dec!(0.5))],
        dec!(500.5); "single lot, fully closed"
    )]
    #[test_case(
        "C-01".into(),
        "P-01".into(),
        "O-01".into(),
        dec!(8.0),
        dec!(2000.0),
        dec!(1200.0),
        dec!(120.0),
        dec!(10.0),
        vec![
            ("L-01", dec!(100.0), dec!(5.0), dec!(0.50)),
            ("L-02", dec!(110.0), dec!(4.0), dec!(0.44)),
        ],
        vec![("L-02", dec!(110.0), dec!(4.0), dec!(0.44), dec!(1.0))],
        vec![("L-01", dec!(100.0), dec!(5.0), dec!(0.50), dec!(0.0))],
        vec![
            ("L-01", dec!(100.0), dec!(5.0), dec!(0.50)),
            ("L-02", dec!(110.0), dec!(3.0), dec!(0.33)),
            ],
        dec!(830.83); "two lots, one closed, one partly closed"
    )]
    fn run_test_batch_asset_position_fully_closed(
        client_order_id: ClientOrderId,
        payment_id: PaymentId,
        order_id: OrderId,
        quantity: Amount,
        remaining_collateral: Amount,
        engaged_collateral: Amount,
        asset_order_price: Amount,
        asset_order_quantity: Amount,
        lots_event_open: Vec<(&str, Amount, Amount, Amount)>,
        lots_expected_open: Vec<(&str, Amount, Amount, Amount, Amount)>,
        lots_expected_closed: Vec<(&str, Amount, Amount, Amount, Amount)>,
        lots_expected_order: Vec<(&str, Amount, Amount, Amount)>,
        expected_collateral_spent: Amount,
    ) {
        let timestamp = Utc::now();

        // this is solver view of what user sent to us (index order)
        let index_order = SolverOrder {
            chain_id: 1,
            address: get_mock_address_1(),
            client_order_id: client_order_id.clone(),
            payment_id: Some(payment_id.clone()),
            symbol: get_mock_asset_name_1(),
            side: Side::Buy,
            remaining_collateral,
            engaged_collateral,
            collateral_carried: dec!(0.0),
            collateral_spent: dec!(0.0),
            filled_quantity: dec!(0.0),
            timestamp,
            status: SolverOrderStatus::Engaged,
            lots: Vec::new(),
        };

        // this is what we sent to exchange (order)
        let order = AssetOrder {
            order_id: order_id.clone(),
            price: asset_order_price,
            quantity: asset_order_quantity,
            side: Side::Buy,
            symbol: get_mock_asset_name_1(),
        };

        // this is what exchange sent to us (fill)
        let lots = lots_event_open
            .iter()
            .map(|(id, p, q, fee)| BatchAssetLot {
                lot_id: id.to_owned().into(),
                order_id: order_id.clone(),
                price: *p,
                original_quantity: *q,
                remaining_quantity: *q,
                fee: *fee,
                closing_lot_id: None,
                timestamp,
            })
            .collect_vec();

        let expected_asset_position_open_lots = lots_expected_open
            .iter()
            .map(|(id, p, q, fee, rq)| BatchAssetLot {
                lot_id: id.to_owned().into(),
                order_id: order_id.clone(),
                price: *p,
                original_quantity: *q,
                remaining_quantity: *rq,
                fee: *fee,
                closing_lot_id: None,
                timestamp,
            })
            .collect_vec();

        let expected_asset_position_closed_lots = lots_expected_closed
            .iter()
            .map(|(id, p, q, fee, rq)| BatchAssetLot {
                lot_id: id.to_owned().into(),
                order_id: order_id.clone(),
                price: *p,
                original_quantity: *q,
                remaining_quantity: *rq,
                fee: *fee,
                closing_lot_id: None,
                timestamp,
            })
            .collect_vec();

        let expected_solver_lots = lots_expected_order
            .iter()
            .map(|(id, p, q, fee)| SolverOrderAssetLot {
                lot_id: id.to_owned().into(),
                symbol: get_mock_asset_name_1(),
                quantity: *q,
                price: *p,
                fee: *fee,
            })
            .collect_vec();

        run_test_batch_asset_position(
            quantity,
            order,
            lots,
            index_order,
            expected_asset_position_open_lots,
            expected_asset_position_closed_lots,
            expected_solver_lots,
            expected_collateral_spent,
        );
    }

    fn run_test_batch_asset_position(
        quantity: Amount,
        order: AssetOrder,
        lots: Vec<BatchAssetLot>,
        mut index_order: SolverOrder,
        expected_asset_position_open_lots: Vec<BatchAssetLot>,
        expected_asset_position_closed_lots: Vec<BatchAssetLot>,
        expected_solver_lots: Vec<SolverOrderAssetLot>,
        expected_collateral_spent: Amount,
    ) {
        let timestamp = Utc::now();
        let tolerance = dec!(0.001);

        let mut asset_position = BatchAssetPosition::try_new(&order, timestamp).unwrap();
        asset_position.realized_value = dec!(500.0);
        asset_position.fee = dec!(0.5);
        asset_position.open_lots = VecDeque::from_iter(lots);
        asset_position.last_update_timestamp = timestamp;

        let (quantity_left, collateral_spent) = asset_position
            .try_allocate_lots(&mut index_order, quantity, tolerance)
            .unwrap();

        assert_decimal_approx_eq!(quantity_left, Amount::ZERO, tolerance);

        let assert_lots_eq = |(a, b): (&BatchAssetLot, &BatchAssetLot)| {
            assert_eq!(a.lot_id, b.lot_id);
            assert_eq!(a.order_id, b.order_id);
            assert_decimal_approx_eq!(a.price, b.price, tolerance);
            assert_decimal_approx_eq!(a.fee, b.fee, tolerance);
            assert_decimal_approx_eq!(a.original_quantity, b.original_quantity, tolerance);
            assert_decimal_approx_eq!(a.remaining_quantity, b.remaining_quantity, tolerance);
        };

        let assert_solver_lots_eq = |(a, b): (&SolverOrderAssetLot, &SolverOrderAssetLot)| {
            assert_eq!(a.lot_id, b.lot_id);
            assert_eq!(a.symbol, b.symbol);
            assert_decimal_approx_eq!(a.price, b.price, tolerance);
            assert_decimal_approx_eq!(a.fee, b.fee, tolerance);
            assert_decimal_approx_eq!(a.quantity, b.quantity, tolerance);
        };

        assert_eq!(
            asset_position.open_lots.len(),
            expected_asset_position_open_lots.len()
        );

        expected_asset_position_open_lots
            .iter()
            .zip(asset_position.open_lots.iter())
            .for_each(assert_lots_eq);

        assert_eq!(
            asset_position.closed_lots.len(),
            expected_asset_position_closed_lots.len()
        );

        expected_asset_position_closed_lots
            .iter()
            .zip(asset_position.closed_lots.iter())
            .for_each(assert_lots_eq);

        expected_solver_lots
            .iter()
            .zip(index_order.lots.iter())
            .for_each(assert_solver_lots_eq);

        assert_decimal_approx_eq!(collateral_spent, expected_collateral_spent, tolerance);
    }

    struct MockHost;

    impl SetSolverOrderStatus for MockHost {
        fn set_order_status(&self, order: &mut SolverOrder, status: SolverOrderStatus) {
            tracing::info!("Set order status: {:?}", status);
            order.status = status;
        }

        fn set_quote_status(&self, order: &mut SolverQuote, status: SolverQuoteStatus) {
            todo!()
        }
    }

    impl BatchManagerHost for MockHost {
        fn get_next_order_id(&self) -> OrderId {
            "O-1".into()
        }

        fn send_order_batch(&self, batch_order: Arc<BatchOrder>) -> Result<()> {
            let _ = batch_order;
            tracing::info!("Send order batch");
            Ok(())
        }

        fn fill_order_request(
            &self,
            chain_id: u32,
            address: &Address,
            client_order_id: &ClientOrderId,
            symbol: &Symbol,
            collateral_spent: Amount,
            fill_amount: Amount,
            timestamp: chrono::DateTime<Utc>,
        ) -> Result<()> {
            let _ = timestamp;
            let _ = fill_amount;
            let _ = collateral_spent;
            let _ = symbol;
            let _ = client_order_id;
            let _ = address;
            let _ = chain_id;
            tracing::info!("Fill order request");
            Ok(())
        }
    }

    /// Batch Manager
    /// ------
    /// Batch Manager manages batches of Index Orders that were previosuly
    /// computed by SolverStrategy plugin (e.g. SimpleSolver). When Solver
    /// receives collateral confirmation from Collateral Manager, it will
    /// place Index Order in Ready state. Then Solver will pick a bunch of
    /// Index Orders that are in Ready state and send it to SolverStrategy,
    /// which will compute what we call here Engagement. An Engagement is
    /// a result of computation by SolverStrategy, and it contains list of
    /// Solver Orders (Solver views of Index Orders w/ Solver state), and
    /// for each of those it contains Asset Contribution Fractions mapping,
    /// which in case we get a fill, tells how much of the executed asset
    /// we should assign to each Index Order.
    ///
    /// Note that we have multiple Index Order bashed together into batch, and
    /// each single asset only gets one order sent to exchange, so we have to
    /// split fills in-between contributing Index Orders following some
    /// distribution, which is defined by Asset Contribution Fractions mapping
    /// for each order. For any given asset, the value of that mapping for all
    /// orders must sum up to 100%.
    ///
    #[test]
    fn test_batch_manager() {
        let timestamp = Utc::now();
        let max_batch_size = 4;
        let zero_threshold = dec!(0.001);
        let fill_threshold = dec!(0.999);
        let mint_threshold = dec!(0.990);
        let mint_wait_period = TimeDelta::seconds(10);

        let host = MockHost;

        let mut batch_manager = BatchManager::new(
            max_batch_size,
            zero_threshold,
            fill_threshold,
            mint_threshold,
            mint_wait_period,
        );

        let (batch_complete_get, batch_complete_set) = get_mock_atomic_bool_pair();

        batch_manager
            .get_single_observer_mut()
            .set_observer_fn(move |e| match e {
                BatchEvent::BatchComplete {
                    batch_order_id,
                    continued_orders,
                } => {
                    assert_eq!(*batch_order_id, "B-1".to_owned());
                    assert_eq!(continued_orders.len(), 1);

                    let first = continued_orders[0].read();
                    assert_eq!(*first.client_order_id, "C-1".to_owned());

                    flag_mock_atomic_bool(&batch_complete_set);
                }
                _ => unreachable!(),
            });

        let index_order = Arc::new(RwLock::new(SolverOrder {
            chain_id: 1,
            address: get_mock_address_1(),
            client_order_id: "C-1".into(),
            payment_id: Some("P-1".into()),
            symbol: get_mock_asset_name_1(),
            side: Side::Buy,
            remaining_collateral: dec!(2000.0),
            engaged_collateral: dec!(1200.0),
            collateral_carried: dec!(0.0),
            collateral_spent: dec!(0.0),
            filled_quantity: dec!(0.0),
            timestamp,
            status: SolverOrderStatus::Engaged,
            lots: Vec::new(),
        }));

        let index_order_clone = index_order.clone();

        let weights = [AssetWeight::new(get_mock_asset_1_arc(), dec!(1.0))];
        let asset_price_limits = HashMap::from([(get_mock_asset_name_1(), dec!(120.0))]);

        let basket_definition = BasketDefinition::try_new(weights).unwrap();

        let basket = Arc::new(
            Basket::new_with_prices(basket_definition, &asset_price_limits, dec!(1200.0)).unwrap(),
        );

        // Engagement definition would be built be SolverStrategy (e.g. SimpleSolver)
        let engagement_definition = Arc::new(RwLock::new(EngagedSolverOrders {
            batch_order_id: "B-1".into(),
            engaged_buys: EngagedSolverOrdersSide {
                asset_price_limits,
                asset_quantities: HashMap::from([(get_mock_asset_name_1(), dec!(10.0))]),
                engaged_orders: vec![SolverOrderEngagement {
                    index_order,
                    asset_contribution_fractions: HashMap::from([(
                        get_mock_asset_name_1(),
                        dec!(1.0),
                    )]),
                    asset_quantity_contributions: HashMap::from([(
                        get_mock_asset_name_1(),
                        dec!(10.0),
                    )]),
                    chain_id: 1,
                    address: get_mock_address_1(),
                    client_order_id: "C-1".into(),
                    symbol: get_mock_asset_name_1(),
                    basket,
                    engaged_side: Side::Buy,
                    engaged_collateral: dec!(1200.0),
                    new_engaged_collateral: dec!(1200.0),
                    engaged_quantity: dec!(1.0),
                    engaged_price: dec!(1200.0),
                    filled_quantity: dec!(0.0),
                }],
            },
        }));

        // Engagement confirmation would be built by Index Order Manager
        let engagement_confirmation = HashMap::from([(
            (get_mock_address_1(), "C-1".into()),
            EngagedIndexOrder {
                chain_id: 1,
                address: get_mock_address_1(),
                client_order_id: "C-1".into(),
                collateral_engaged: dec!(1200.0),
                collateral_remaining: dec!(2000.0),
            },
        )]);

        // that will store engagement in cache - required initial step
        batch_manager
            .handle_new_engagement(engagement_definition)
            .unwrap();

        // confirms engagements with index order manager - updates cached index order
        batch_manager
            .handle_engage_index_order(&host, "B-1".into(), engagement_confirmation, timestamp)
            .unwrap();

        // sends more batches - todo: check grouping and coalescing by asset
        batch_manager.process_batches(&host, timestamp).unwrap();

        // each fill opens new or closes existing lot in the inventory -
        // we should match against index order engagements following the
        // contribution fractions
        batch_manager
            .handle_new_lot(
                &host,
                "O-1".into(),
                "B-1".into(),
                "L-1".into(),
                None,
                get_mock_asset_name_1(),
                Side::Buy,
                dec!(100.0),
                dec!(5.0),
                dec!(0.5),
                timestamp,
            )
            .unwrap();

        //
        batch_manager
            .handle_cancel_order(
                &host,
                "B-1".into(),
                get_mock_asset_name_1(),
                Side::Buy,
                dec!(5.0),
                true,
                timestamp,
            )
            .unwrap();

        let index_order_read = index_order_clone.read();

        assert_decimal_approx_eq!(
            index_order_read.remaining_collateral,
            dec!(2000.0),
            zero_threshold
        );
        assert_decimal_approx_eq!(
            index_order_read.engaged_collateral,
            dec!(0.0),
            zero_threshold
        );
        assert_decimal_approx_eq!(
            index_order_read.collateral_carried,
            dec!(699.50),
            zero_threshold
        );
        assert_decimal_approx_eq!(
            index_order_read.collateral_spent,
            dec!(500.50),
            zero_threshold
        );
        assert_decimal_approx_eq!(index_order_read.filled_quantity, dec!(0.5), zero_threshold);

        let assert_solver_lots_eq = |(a, b): (&SolverOrderAssetLot, &SolverOrderAssetLot)| {
            assert_eq!(a.lot_id, b.lot_id);
            assert_eq!(a.symbol, b.symbol);
            assert_decimal_approx_eq!(a.price, b.price, zero_threshold);
            assert_decimal_approx_eq!(a.fee, b.fee, zero_threshold);
            assert_decimal_approx_eq!(a.quantity, b.quantity, zero_threshold);
        };

        let expected_solver_lots = [SolverOrderAssetLot {
            lot_id: "L-1".into(),
            symbol: get_mock_asset_name_1(),
            quantity: dec!(5.0),
            price: dec!(100.0),
            fee: dec!(0.5),
        }];

        expected_solver_lots
            .iter()
            .zip(index_order_read.lots.iter())
            .for_each(assert_solver_lots_eq);

        assert!(test_mock_atomic_bool(&batch_complete_get));
    }
}
