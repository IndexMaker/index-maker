use std::{collections::HashMap, sync::Arc};

use alloy::transports::http::reqwest::Response;
use chrono::{DateTime, Utc};
use eyre::{eyre, OptionExt, Result};
use parking_lot::RwLock;
use safe_math::safe;

use crate::{
    core::{
        bits::{BatchOrderId, PaymentId},
        decimal_ext::DecimalExt,
        functional::{IntoObservableSingle, PublishSingle, SingleObserver},
    },
    server::server::{Server, ServerEvent},
    solver::index_order::{self, IndexOrder},
};

use crate::core::bits::{Address, Amount, ClientOrderId, Side, Symbol};

use super::index_order::{CancelIndexOrderOutcome, UpdateIndexOrderOutcome};

pub struct EngagedIndexOrder {
    // ID of the original NewOrder request
    pub original_client_order_id: ClientOrderId,

    /// On-chain address of the User
    pub address: Address,

    // ID of the NewOrder request
    pub client_order_id: ClientOrderId,

    /// Quantity remaining
    pub quantity_engaged: Amount,

    /// Quantity remaining
    pub quantity_remaining: Amount,
}

pub enum IndexOrderEvent {
    NewIndexOrder {
        // ID of the original NewOrder request
        original_client_order_id: ClientOrderId,

        /// On-chain address of the User
        address: Address,

        // ID of the NewOrder request
        client_order_id: ClientOrderId,

        /// An ID of the on-chain payment
        payment_id: PaymentId,

        /// Symbol of an Index
        symbol: Symbol,

        /// Side of an order
        side: Side,

        /// Limit price
        price: Amount,

        /// Price max deviation %-age (as fraction) threshold
        price_threshold: Amount,

        /// Quantity of index requested
        quantity: Amount,

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
    UpdateIndexOrder {
        // ID of the original NewOrder request
        original_client_order_id: ClientOrderId,

        /// On-chain address of the User
        address: Address,

        // ID of the NewOrder request
        client_order_id: ClientOrderId,

        /// Quantity removed
        quantity_removed: Amount,

        /// Quantity remaining
        quantity_remaining: Amount,

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
        address: Address,
        client_order_id: ClientOrderId,
        payment_id: PaymentId,
        symbol: Symbol,
        side: Side,
        price: Amount,
        price_threshold: Amount,
        quantity: Amount,
        timestamp: DateTime<Utc>,
    ) -> Result<()> {
        // Create index orders for user if not created yet
        let user_index_orders = self
            .index_orders
            .entry(address)
            .or_insert_with(|| HashMap::new());

        // Create index order for if not created yet
        let index_order = user_index_orders
            .entry(symbol.clone())
            .or_insert_with(|| {
                let order = Arc::new(RwLock::new(IndexOrder::new(
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

        // Add update to index order
        let update_order_outcome = index_order.write().update_order(
            address.clone(),
            client_order_id.clone(),
            payment_id.clone(),
            side,
            price,
            price_threshold,
            quantity,
            timestamp,
            self.tolerance,
        )?;

        match update_order_outcome {
            UpdateIndexOrderOutcome::Push { new_quantity } => {
                self.observer
                    .publish_single(IndexOrderEvent::NewIndexOrder {
                        original_client_order_id,
                        address,
                        client_order_id: client_order_id.clone(),
                        payment_id,
                        symbol,
                        side,
                        price,
                        price_threshold,
                        quantity: new_quantity,
                        timestamp,
                    });
            }
            UpdateIndexOrderOutcome::Reduce {
                removed_quantity,
                remaining_quantity,
            } => {
                self.observer
                    .publish_single(IndexOrderEvent::UpdateIndexOrder {
                        original_client_order_id,
                        address,
                        client_order_id: client_order_id.clone(),
                        quantity_removed: removed_quantity,
                        quantity_remaining: remaining_quantity,
                        timestamp,
                    });
            }
            UpdateIndexOrderOutcome::Flip { side, new_quantity } => {
                self.observer
                    .publish_single(IndexOrderEvent::CancelIndexOrder {
                        original_client_order_id: original_client_order_id.clone(),
                        address,
                        client_order_id: client_order_id.clone(),
                        timestamp,
                    });
                self.observer
                    .publish_single(IndexOrderEvent::NewIndexOrder {
                        original_client_order_id,
                        address,
                        client_order_id: client_order_id.clone(),
                        payment_id,
                        symbol,
                        side,
                        price,
                        price_threshold,
                        quantity: new_quantity,
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
        payment_id: PaymentId,
        symbol: Symbol,
        quantity: Amount,
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
            .cancel_updates(quantity, self.tolerance)?
        {
            CancelIndexOrderOutcome::Cancel {
                removed_quantity: _,
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
                removed_quantity,
                remaining_quantity,
            } => {
                self.observer
                    .publish_single(IndexOrderEvent::UpdateIndexOrder {
                        original_client_order_id,
                        address,
                        client_order_id,
                        quantity_removed: removed_quantity,
                        quantity_remaining: remaining_quantity,
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
                address,
                client_order_id,
                payment_id,
                symbol,
                side,
                price,
                price_threshold,
                quantity,
                timestamp,
            } => self.new_index_order(
                address.clone(),
                client_order_id.clone(),
                payment_id.clone(),
                symbol.clone(),
                *side,
                *price,
                *price_threshold,
                *quantity,
                timestamp.clone(),
            ),
            ServerEvent::CancelIndexOrder {
                address,
                client_order_id,
                payment_id,
                symbol,
                quantity,
                timestamp,
            } => self.cancel_index_order(
                address.clone(),
                client_order_id.clone(),
                payment_id.clone(),
                symbol.clone(),
                *quantity,
                timestamp.clone(),
            ),
            _ => Ok(()),
        }
    }

    /// provide a method to fill index order request
    pub fn fill_order_request(
        &mut self,
        address: &Address,
        client_order_id: &ClientOrderId,
        symbol: &Symbol,
        fill_amount: Amount,
        timestamp: DateTime<Utc>,
    ) -> Result<()> {
        self.index_orders
            .get(address)
            .and_then(|x| x.get(symbol))
            .and_then(|index_order| Some(index_order.write()))
            .and_then(|mut index_order| {
                // TOOD: We need to fill the updates too!
                index_order.filled_quantity = safe!(index_order.filled_quantity + fill_amount)?;
                index_order.engaged_quantity =
                    Some(safe!(index_order.engaged_quantity? - fill_amount)?);
                println!(
                    "IndexOrderManager: Fill: {} {:0.5} (+{:0.5}), Remainig Quantity: {:0.5}",
                    client_order_id,
                    index_order.filled_quantity,
                    fill_amount,
                    index_order.remaining_quantity
                );

                let quantity_remaining =
                    safe!(index_order.engaged_quantity + index_order.remaining_quantity)?;

                self.server.write().respond_with(
                    crate::server::server::ServerResponse::IndexOrderFill {
                        address: *address,
                        client_order_id: client_order_id.clone(),
                        filled_quantity: index_order.filled_quantity,
                        quantity_remaining,
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
        engaged_orders: Vec<(Address, ClientOrderId, Symbol, Amount)>,
    ) -> Result<()> {
        let mut engage_result = HashMap::new();
        for (address, client_order_id, symbol, quantity) in engaged_orders {
            if let Some(index_order) = self
                .index_orders
                .get_mut(&address)
                .and_then(|map| map.get_mut(&symbol))
            {
                let mut index_order = index_order.write();
                let unmatched_quantity = index_order.solver_engage(quantity, self.tolerance)?;
                println!(
                    "IndexOrderManager: Engage {} {:0.5} {:0.5} {:0.5}",
                    client_order_id,
                    quantity,
                    index_order.engaged_quantity.unwrap_or_default(),
                    index_order.remaining_quantity
                );

                let quantity_engaged = if let Some(unmatched_quantity) = unmatched_quantity {
                    safe!(quantity - unmatched_quantity)
                } else {
                    Some(quantity)
                };

                if let Some(quantity_engaged) = quantity_engaged {
                    engage_result.insert(
                        (address, client_order_id.clone()),
                        EngagedIndexOrder {
                            original_client_order_id: index_order.original_client_order_id.clone(),
                            address,
                            client_order_id: client_order_id.clone(),
                            quantity_engaged,
                            quantity_remaining: index_order.remaining_quantity,
                        },
                    );
                } else {
                    index_order.solver_cancel("Math overflow");
                }
            } else {
                return Err(eyre!("No such index order {} {}", address, symbol));
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
