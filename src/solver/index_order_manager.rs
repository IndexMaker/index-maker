use std::{collections::HashMap, sync::Arc};

use chrono::{DateTime, Utc};
use eyre::{eyre, OptionExt, Result};
use itertools::Itertools;
use parking_lot::RwLock;
use safe_math::safe;

use crate::{
    core::{
        bits::{BatchOrderId, PaymentId},
        decimal_ext::DecimalExt,
        functional::{IntoObservableSingle, PublishSingle, SingleObserver},
    },
    server::server::{Server, ServerEvent, ServerResponse},
    solver::index_order::IndexOrder,
};

use crate::core::bits::{Address, Amount, ClientOrderId, Side, Symbol};

use super::index_order::{self, CancelIndexOrderOutcome, UpdateIndexOrderOutcome};

pub struct EngageOrderRequest {
    pub address: Address,
    pub client_order_id: ClientOrderId,
    pub symbol: Symbol,
    pub collateral_amount: Amount,
}

pub struct EngagedIndexOrder {
    // ID of the original NewOrder request
    pub original_client_order_id: ClientOrderId,

    /// On-chain address of the User
    pub address: Address,

    // ID of the NewOrder request
    pub client_order_id: ClientOrderId,

    /// Quantity remaining
    pub collateral_engaged: Amount,

    /// Quantity remaining
    pub collateral_remaining: Amount,
}

pub enum IndexOrderEvent {
    NewIndexOrder {
        // Chain ID
        chain_id: u32,

        // ID of the original NewOrder request
        original_client_order_id: ClientOrderId,

        /// On-chain address of the User
        address: Address,

        // ID of the NewOrder request
        client_order_id: ClientOrderId,

        /// Symbol of an Index
        symbol: Symbol,

        /// Side of an order
        side: Side,

        /// Collateral to spend
        collateral_amount: Amount,

        /// Time when requested
        timestamp: DateTime<Utc>,
    },
    EngageIndexOrder {
        // ID of the batch of engagement
        batch_order_id: BatchOrderId,

        // A set of index orders in the engagement batch
        engaged_orders: HashMap<(Address, ClientOrderId), EngagedIndexOrder>,

        /// Time when requested
        timestamp: DateTime<Utc>,
    },
    CollateralReady {
        // ID of the original NewOrder request
        original_client_order_id: ClientOrderId,

        /// On-chain address of the User
        address: Address,

        // ID of the NewOrder request
        client_order_id: ClientOrderId,

        /// Quantity remaining
        collateral_remaining: Amount,

        /// Quantity spent
        collateral_spent: Amount,

        /// Quantity in fees
        fees: Amount,

        /// Time when requested
        timestamp: DateTime<Utc>,
    },
    UpdateIndexOrder {
        // ID of the original NewOrder request
        original_client_order_id: ClientOrderId,

        /// On-chain address of the User
        address: Address,

        // ID of the NewOrder request
        client_order_id: ClientOrderId,

        /// Quantity removed
        collateral_removed: Amount,

        /// Quantity remaining
        collateral_remaining: Amount,

        /// Time when requested
        timestamp: DateTime<Utc>,
    },
    CancelIndexOrder {
        // ID of the original NewOrder request
        original_client_order_id: ClientOrderId,

        /// On-chain address of the User
        address: Address,

        /// ID of the Cancel request
        client_order_id: ClientOrderId,

        /// Tell the time when it was cancelled
        timestamp: DateTime<Utc>,
    },
}

/// Manages Incoming Index Orders
///
/// Responsible for pre-processing incomming index orders, so that Solver
/// will only receive resulting order quantity, e.g. A Sell order would be
/// self-matched to remove some of the quantity, and if order was not engaged
/// yet by Solver, then Sell of more quantity than Buy would flip the side
/// of the order to Sell with overflowing quantity. Solver will not engage
/// the order when it cannot obtain liquidity from the market, or also when
/// no sufficient curator token has been received.
pub struct IndexOrderManager {
    observer: SingleObserver<IndexOrderEvent>,
    server: Arc<RwLock<dyn Server>>,
    index_orders: HashMap<Address, HashMap<Symbol, Arc<RwLock<IndexOrder>>>>,
    tolerance: Amount,
}

/// manage index orders, receive orders and route into solver
impl IndexOrderManager {
    pub fn new(server: Arc<RwLock<dyn Server>>, tolerance: Amount) -> Self {
        Self {
            observer: SingleObserver::new(),
            server,
            index_orders: HashMap::new(),
            tolerance,
        }
    }

    /// We would've received NewOrder request from FIX server, and
    /// this can be Buy or Sell Index.
    fn new_index_order(
        &mut self,
        chain_id: u32,
        address: Address,
        client_order_id: ClientOrderId,
        symbol: Symbol,
        side: Side,
        collateral_amount: Amount,
        timestamp: DateTime<Utc>,
    ) -> Result<()> {
        // Create index orders for user if not created yet
        let user_index_orders = self
            .index_orders
            .entry(address)
            .or_insert_with(|| HashMap::new());

        // Create index order if not created yet
        let index_order = user_index_orders
            .entry(symbol.clone())
            .or_insert_with(|| {
                let order = Arc::new(RwLock::new(IndexOrder::new(
                    chain_id,
                    address.clone(),
                    client_order_id.clone(),
                    symbol.clone(),
                    side,
                    timestamp.clone(),
                )));
                order
            })
            .clone();

        let original_client_order_id = index_order.read().original_client_order_id.clone();

        if !index_order.read().order_updates.is_empty() {
            // TODO: Our current support for order updates is limitted, i.e. we
            // can accept updates, but then neither Solver nor CollateralManager
            // will be able to handle them correctly yet. Also we don't handle
            // correctly fills here should order have more updates.
            Err(eyre!("{} - {}",
                "We currently cannot support order updates",
                "IndexOrder must be fully processed before any next order"))?;
        }

        // Add update to index order
        let update_order_outcome = index_order.write().update_order(
            client_order_id.clone(),
            side,
            collateral_amount,
            timestamp,
            self.tolerance,
        )?;

        match update_order_outcome {
            UpdateIndexOrderOutcome::Push {
                new_collateral_amount,
            } => {
                self.observer
                    .publish_single(IndexOrderEvent::NewIndexOrder {
                        chain_id,
                        original_client_order_id,
                        address,
                        client_order_id: client_order_id.clone(),
                        symbol,
                        side,
                        collateral_amount: new_collateral_amount,
                        timestamp,
                    });
            }
            UpdateIndexOrderOutcome::Reduce {
                removed_collateral,
                remaining_collateral,
            } => {
                self.observer
                    .publish_single(IndexOrderEvent::UpdateIndexOrder {
                        original_client_order_id,
                        address,
                        client_order_id: client_order_id.clone(),
                        collateral_removed: removed_collateral,
                        collateral_remaining: remaining_collateral,
                        timestamp,
                    });
            }
            UpdateIndexOrderOutcome::Flip {
                side,
                new_collateral_amount,
            } => {
                self.observer
                    .publish_single(IndexOrderEvent::CancelIndexOrder {
                        original_client_order_id: original_client_order_id.clone(),
                        address,
                        client_order_id: client_order_id.clone(),
                        timestamp,
                    });
                self.observer
                    .publish_single(IndexOrderEvent::NewIndexOrder {
                        chain_id,
                        original_client_order_id,
                        address,
                        client_order_id: client_order_id.clone(),
                        symbol,
                        side,
                        collateral_amount: new_collateral_amount,
                        timestamp,
                    });
            }
        };

        self.server
            .write()
            .respond_with(crate::server::server::ServerResponse::NewIndexOrderAck {
                address,
                client_order_id,
                timestamp,
            });

        Ok(())
    }

    /// We would've received CancelOrder request from FIX server.
    fn cancel_index_order(
        &self,
        address: Address,
        client_order_id: ClientOrderId,
        symbol: Symbol,
        collateral_amount: Amount,
        timestamp: DateTime<Utc>,
    ) -> Result<()> {
        let user_orders = self
            .index_orders
            .get(&address)
            .ok_or(eyre!("No orders found for user {}", address))?;
        let index_order = user_orders.get(&symbol).ok_or(eyre!(
            "No order found for user {} for {}",
            address,
            symbol
        ))?;

        let original_client_order_id = index_order.read().original_client_order_id.clone();

        match index_order
            .write()
            .cancel_updates(collateral_amount, self.tolerance)?
        {
            CancelIndexOrderOutcome::Cancel {
                removed_collateral: _,
            } => {
                self.observer
                    .publish_single(IndexOrderEvent::CancelIndexOrder {
                        original_client_order_id,
                        address,
                        client_order_id,
                        timestamp,
                    });
            }
            CancelIndexOrderOutcome::Reduce {
                removed_collateral,
                remaining_collateral,
            } => {
                self.observer
                    .publish_single(IndexOrderEvent::UpdateIndexOrder {
                        original_client_order_id,
                        address,
                        client_order_id,
                        collateral_removed: removed_collateral,
                        collateral_remaining: remaining_collateral,
                        timestamp,
                    });
            }
        }
        Ok(())
    }

    /// receive index order requests from (FIX) Server
    pub fn handle_server_message(&mut self, notification: &ServerEvent) -> Result<()> {
        match notification {
            ServerEvent::NewIndexOrder {
                chain_id,
                address,
                client_order_id,
                symbol,
                side,
                collateral_amount,
                timestamp,
            } => self.new_index_order(
                *chain_id,
                address.clone(),
                client_order_id.clone(),
                symbol.clone(),
                *side,
                *collateral_amount,
                timestamp.clone(),
            ),
            ServerEvent::CancelIndexOrder {
                address,
                client_order_id,
                symbol,
                collateral_amount,
                timestamp,
            } => self.cancel_index_order(
                address.clone(),
                client_order_id.clone(),
                symbol.clone(),
                *collateral_amount,
                timestamp.clone(),
            ),
            _ => Ok(()),
        }
    }

    pub fn collateral_ready(
        &mut self,
        address: &Address,
        client_order_id: &ClientOrderId,
        symbol: &Symbol,
        collateral_amount: Amount,
        fees: Amount,
        timestamp: DateTime<Utc>,
    ) -> Result<()> {
        self.index_orders
            .get(address)
            .and_then(|x| x.get(symbol))
            .and_then(|index_order| Some(index_order.upgradable_read()))
            .and_then(|mut order_upread| {
                let (update_remaining_collateral, update_collateral_spent, update_fee) = (|| {
                    let update = order_upread
                        .order_updates
                        .iter()
                        .find(|update| update.read().client_order_id.eq(client_order_id))?;
                    let mut update_upread = update.upgradable_read();

                    let update_collateral_spent = safe!(update_upread.collateral_spent + fees)?;
                    let update_fee = safe!(update_upread.update_fee + fees)?;
                    let update_remaining_collateral =
                        safe!(update_upread.remaining_collateral - fees)?;

                    if update_remaining_collateral < safe!(collateral_amount - self.tolerance)? {
                        eprintln!(
                            "(index-order-manager) Error updating collateral ready: {:0.5} < {:0.5}",
                            update_remaining_collateral, collateral_amount
                        );
                        return None;
                    }

                    update_upread.with_upgraded(|update_write| {
                        update_write.collateral_spent = update_collateral_spent;
                        update_write.update_fee = update_fee;
                        update_write.remaining_collateral = update_remaining_collateral;
                        update_write.timestamp = timestamp;
                    });
                    Some((update_remaining_collateral, update_collateral_spent, update_fee))
                })()?;

                let order_remaining_collateral = safe!(order_upread.remaining_collateral - fees)?;
                let order_collateral_spent = safe!(order_upread.collateral_spent + fees)?;
                let order_fees = safe!(order_upread.fees + fees)?;

                order_upread.with_upgraded(|order_write| {
                    order_write.remaining_collateral = order_remaining_collateral;
                    order_write.collateral_spent = order_collateral_spent;
                    order_write.fees = order_fees;
                    order_write.last_update_timestamp = timestamp;
                });

                self.observer
                    .publish_single(IndexOrderEvent::CollateralReady {
                        original_client_order_id: order_upread.original_client_order_id.clone(),
                        address: *address,
                        client_order_id: client_order_id.clone(),
                        collateral_remaining: update_remaining_collateral,
                        collateral_spent: update_collateral_spent,
                        fees: update_fee,
                        timestamp,
                    });

                Some(())
            })
            .ok_or_eyre("Failed to update index order")?;
        Ok(())
    }

    /// provide a method to fill index order request
    pub fn fill_order_request(
        &mut self,
        address: &Address,
        client_order_id: &ClientOrderId,
        symbol: &Symbol,
        collateral_spent: Amount,
        fill_amount: Amount,
        timestamp: DateTime<Utc>,
    ) -> Result<()> {
        self.index_orders
            .get(address)
            .and_then(|x| x.get(symbol))
            .and_then(|index_order| Some(index_order.write()))
            .and_then(|mut index_order| {
                // TODO: We need to fill the updates too!
                index_order.filled_quantity = safe!(index_order.filled_quantity + fill_amount)?;
                index_order.collateral_spent = safe!(index_order.collateral_spent + collateral_spent)?;
                index_order.engaged_collateral =
                    Some(safe!(index_order.engaged_collateral? - collateral_spent)?);
                println!(
                    "(index-order-manager) Fill: {} {:0.5} (+{:0.5}), Remaining Collateral: {:0.5} + {:0.5} (-{:0.5})",
                    client_order_id,
                    index_order.filled_quantity,
                    fill_amount,
                    index_order.remaining_collateral,
                    index_order.engaged_collateral.unwrap_or_default(),
                    index_order.collateral_spent,
                );

                let collateral_remaining =
                    safe!(index_order.engaged_collateral + index_order.remaining_collateral)?;

                self.server.write().respond_with(
                    ServerResponse::IndexOrderFill {
                        address: *address,
                        client_order_id: client_order_id.clone(),
                        filled_quantity: index_order.filled_quantity,
                        collateral_spent: index_order.collateral_spent,
                        collateral_remaining,
                        timestamp,
                    },
                );

                // TODO: Fire UpdateIndexOrder event!

                Some(())
            })
            .ok_or_eyre("Failed to update index order")?;
        Ok(())
    }

    pub fn engage_orders(
        &mut self,
        batch_order_id: BatchOrderId,
        engaged_orders: Vec<EngageOrderRequest>,
    ) -> Result<()> {
        let mut engage_result = HashMap::new();
        for engage_order in engaged_orders {
            if let Some(index_order) = self
                .index_orders
                .get_mut(&engage_order.address)
                .and_then(|map| map.get_mut(&engage_order.symbol))
            {
                let mut index_order = index_order.write();
                let unmatched_collateral =
                    index_order.solver_engage(engage_order.collateral_amount, self.tolerance)?;
                println!(
                    "(index-order-manager) Engage {} eca=+{:0.5} iec={:0.5} irc={:0.5}",
                    engage_order.client_order_id,
                    engage_order.collateral_amount,
                    index_order.engaged_collateral.unwrap_or_default(),
                    index_order.remaining_collateral
                );

                let collateral_engaged = if let Some(unmatched_collateral) = unmatched_collateral {
                    safe!(engage_order.collateral_amount - unmatched_collateral)
                } else {
                    Some(engage_order.collateral_amount)
                };

                if let Some(collateral_engaged) = collateral_engaged {
                    engage_result.insert(
                        (engage_order.address, engage_order.client_order_id.clone()),
                        EngagedIndexOrder {
                            original_client_order_id: index_order.original_client_order_id.clone(),
                            address: engage_order.address,
                            client_order_id: engage_order.client_order_id.clone(),
                            collateral_engaged,
                            collateral_remaining: index_order.remaining_collateral,
                        },
                    );
                } else {
                    index_order.solver_cancel("Math overflow");
                }
            } else {
                return Err(eyre!(
                    "No such index order {} {}",
                    engage_order.address,
                    engage_order.symbol
                ));
            }
        }
        self.observer
            .publish_single(IndexOrderEvent::EngageIndexOrder {
                batch_order_id: batch_order_id.clone(),
                engaged_orders: engage_result,
                timestamp: Utc::now(),
            });
        Ok(())
    }
}

impl IntoObservableSingle<IndexOrderEvent> for IndexOrderManager {
    fn get_single_observer_mut(&mut self) -> &mut SingleObserver<IndexOrderEvent> {
        &mut self.observer
    }
}
