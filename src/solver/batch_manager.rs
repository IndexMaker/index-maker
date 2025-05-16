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
    core::{
        bits::{
            Address, Amount, AssetOrder, BatchOrder, BatchOrderId, ClientOrderId, OrderId, Side,
            Symbol,
        },
        decimal_ext::DecimalExt,
    },
    solver::solver::SolverOrderStatus,
};

use super::{
    index_order_manager::{EngagedIndexOrder, IndexOrderManager},
    inventory_manager::InventoryManager,
    position::LotId,
    solver::{
        EngagedSolverOrders, SetSolverOrderStatus, SolverOrder, SolverOrderAssetLot,
        SolverOrderEngagement,
    },
};

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
    fn try_build_solver_asset_lot(
        &self,
        symbol: Symbol,
        quantity: Amount,
    ) -> Option<SolverOrderAssetLot> {
        Some(SolverOrderAssetLot {
            symbol,
            quantity,
            price: self.price,
            fee: safe!(self.fee * safe!(quantity / self.original_quantity)?)?,
        })
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
    ) -> Option<(Amount, Amount)> {
        let mut collateral_spent = Amount::ZERO;
        let mut push_allocation = |lot: &mut BatchAssetLot, quantity: Amount| -> Option<()> {
            let asset_allocation = lot.try_build_solver_asset_lot(self.symbol.clone(), quantity)?;
            let lot_collateral_spent = asset_allocation.compute_collateral_spent()?;
            collateral_spent = safe!(collateral_spent + lot_collateral_spent)?;
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

pub trait BatchManagerHost: SetSolverOrderStatus {
    fn get_next_order_id(&self) -> OrderId;
    fn handle_orders_ready(&self, ready_orders: VecDeque<Arc<RwLock<SolverOrder>>>);
    fn send_order_batch(&self, batch_order: Arc<BatchOrder>) -> Result<()>;
    fn fill_order_request(
        &self,
        address: &Address,
        client_order_id: &ClientOrderId,
        symbol: &Symbol,
        collateral_spent: Amount,
        fill_amount: Amount,
        timestamp: DateTime<Utc>,
    ) -> Result<()>;
}

pub struct BatchManager {
    batches: RwLock<HashMap<BatchOrderId, BatchOrderStatus>>,
    engagements: RwLock<HashMap<BatchOrderId, EngagedSolverOrders>>,
    ready_batches: Mutex<VecDeque<BatchOrderId>>,
    ready_mints: Mutex<VecDeque<Arc<RwLock<SolverOrder>>>>,
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
            engagements: RwLock::new(HashMap::new()),
            ready_batches: Mutex::new(VecDeque::new()),
            batches: RwLock::new(HashMap::new()),
            ready_mints: Mutex::new(VecDeque::new()),
            max_batch_size,
            zero_threshold,
            fill_threshold,
            mint_threshold,
            mint_wait_period,
        }
    }

    fn send_batch(
        &self,
        host: &dyn BatchManagerHost,
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
                                order_id: host.get_next_order_id(),
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

            host.send_order_batch(batch)?;
        }
        Ok(())
    }

    fn fill_index_order(
        &self,
        host: &dyn BatchManagerHost,
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
                "(batch-manager) Fill Basket Asset: {:5} q={:0.5} pos={:0.5} aq={:0.5} cf={:0.5} afr={:0.5}",
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
        let mut collateral_spent = Amount::ZERO;
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
            let asset_collateral_spent = match position
                .try_allocate_lots(&mut index_order_write, asset_quantity)
            {
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
        engaged_order.with_upgraded(|x| {
            x.filled_quantity = filled_quantity;
        });

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

        println!(
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
            &index_order_write.address,
            &index_order_write.original_client_order_id,
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
            host.set_order_status(&mut index_order_write, SolverOrderStatus::FullyMintable);
        } else if self.fill_threshold < fill_rate {
            index_order_write.collateral_carried = index_order_write.engaged_collateral;
            index_order_write.engaged_collateral = Amount::ZERO;
            return Ok(Some(index_order.clone()));
        }

        Ok(None)
    }

    fn fill_batch_orders(
        &self,
        host: &dyn BatchManagerHost,
        batch: &mut BatchOrderStatus,
    ) -> Result<()> {
        let engagements_read = self.engagements.read();
        let engagement = engagements_read
            .get(&batch.batch_order_id)
            .ok_or_else(|| eyre!("Engagement not found {}", batch.batch_order_id))?;

        let mut ready_orders = VecDeque::new();

        for engaged_order in &engagement.engaged_orders {
            if let Some(index_order) = self.fill_index_order(host, batch, engaged_order)? {
                ready_orders.push_back(index_order);
            }
        }

        host.handle_orders_ready(ready_orders);
        Ok(())
    }

    pub fn send_more_batches(
        &self,
        host: &dyn BatchManagerHost,
        timestamp: DateTime<Utc>,
    ) -> Result<()> {
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
            self.send_batch(host, engaged_orders, timestamp)?;
        }

        Ok(())
    }

    pub fn get_mintable_batch(&self, timestamp: DateTime<Utc>) -> Vec<Arc<RwLock<SolverOrder>>> {
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
        ready_mints
    }

    pub fn handle_new_engagement(
        &self,
        engaged_orders: EngagedSolverOrders,
    ) -> Result<BatchOrderId> {
        let batch_order_id = match self
            .engagements
            .write()
            .entry(engaged_orders.batch_order_id.clone())
        {
            Entry::Vacant(entry) => entry.insert(engaged_orders).batch_order_id.clone(),
            Entry::Occupied(_) => Err(eyre!(
                "Dublicate Batch Id: {}",
                engaged_orders.batch_order_id
            ))?,
        };
        Ok(batch_order_id)
    }

    pub fn handle_engage_index_order(
        &self,
        host: &dyn BatchManagerHost,
        batch_order_id: BatchOrderId,
        engaged_orders: HashMap<(Address, ClientOrderId), EngagedIndexOrder>,
        timestamp: DateTime<Utc>,
    ) -> Result<()> {
        println!(
            "\n(batch-manager) Handle Index Order EngageIndexOrder {}",
            batch_order_id
        );
        match self.engagements.write().get(&batch_order_id) {
            Some(engaged_orders_stored) => {
                engaged_orders_stored
                    .engaged_orders
                    .iter()
                    .for_each(|engaged_order_stored| {
                        let mut engaged_order_stored = engaged_order_stored.write();

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

                                match index_order_stored.status {
                                    SolverOrderStatus::Ready => {
                                        host.set_order_status(
                                            &mut index_order_stored,
                                            SolverOrderStatus::Engaged,
                                        );
                                    }
                                    _ => (),
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
                self.ready_batches.lock().push_back(batch_order_id);
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
        symbol: Symbol,
        side: Side,
        price: Amount,
        quantity: Amount,
        fee: Amount,
        timestamp: DateTime<Utc>,
    ) -> Result<()> {
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
                "(batch-manager) Batch Position: {:?} {:5} total={:0.5} volley={:0.5} pos={:0.5} real={:0.5} + fee={:0.5}",
                position.side,
                position.symbol,
                position.order_quantity,
                position.volley_size,
                position.position,
                position.realized_value,
                position.fee
            );

            println!(
                "(batch-manager) Batch Status: {} volley={:0.5} fill={:0.5} frac={:0.5} real={:0.5} fee={:0.5}",
                batch_order_id,
                batch.volley_size,
                batch.filled_volley,
                batch.filled_fraction,
                batch.realized_value,
                batch.fee
            );
            Some(())
        })()
        .ok_or_eyre("Math Problem")?;

        batch.last_update_timestamp = timestamp;

        self.fill_batch_orders(host, batch)
    }
}
