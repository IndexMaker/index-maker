use std::{
    collections::{HashMap, VecDeque},
    sync::Arc,
};

use chrono::{DateTime, Utc};
use eyre::{eyre, Result};
use parking_lot::RwLock;

use crate::{
    core::{bits::PaymentId, functional::SingleObserver},
    server::server::{Server, ServerEvent},
    solver::index_order::IndexOrder,
};

use crate::core::bits::{Address, Amount, ClientOrderId, Side, Symbol};

use super::index_order::IndexOrderUpdate;

pub enum IndexOrderEvent {
    NewIndexOrder {
        /// On-chain address of the User
        address: Address,

        // ID od the NewOrder request
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

        /// Quantity removed (on opposite side) from existing index order as a result of crossing sides
        quantity_removed: Amount,

        /// Quantity either created or added to an index order
        quantity_added: Amount,

        /// Time when requested
        timestamp: DateTime<Utc>,
    },
    CancelIndexOrder {
        /// On-chain address of the User
        address: Address,
        
        /// ID of the Cancel request
        client_order_id: ClientOrderId,
        
        /// An ID of the on-chain payment
        payment_id: PaymentId,
        
        /// Symbol of an Index
        symbol: Symbol,

        /// Tell if index order was fully cancelled
        order_cancelled: bool,

        /// Tell on which side was the index order when it was cancelled
        side_cancelled: Side,

        /// Tell the quantity of the index order that was cancelled
        quantity_cancelled: Amount,

        /// Tell the time when it was cancelled
        timestamp: DateTime<Utc>,
    },
}

pub struct IndexOrderManager {
    pub observer: SingleObserver<IndexOrderEvent>,
    pub server: Arc<RwLock<dyn Server>>,
    pub index_orders: HashMap<Address, HashMap<Symbol, Arc<RwLock<IndexOrder>>>>,
    pub tolerance: Amount,
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

    fn match_index_order_updates(
        &self,
        index_order: &mut IndexOrder,
        mut remaining_quantity: Amount,
    ) -> Result<Option<Amount>> {
        while let Some(update) = index_order.order_updates.front_mut() {
            // quantity remaining on the update
            let quantity_remaining = update
                .remaining_quantity
                .checked_sub(remaining_quantity)
                .ok_or(eyre!("Math overflow"))?;

            if quantity_remaining < self.tolerance {
                // close that update
                let cloned = update.clone();
                index_order.closed_updates.push_back(cloned);
                index_order.order_updates.pop_front();

                if quantity_remaining < -self.tolerance {
                    // there's more quantity on the incoming update left
                    remaining_quantity = -quantity_remaining;
                } else {
                    // we consumed whole incoming update
                    return Ok(None);
                }
            }
        }
        Ok(Some(remaining_quantity))
    }

    fn update_index_order(
        &self,
        index_order: Arc<RwLock<IndexOrder>>,
        mut index_order_update: IndexOrderUpdate,
    ) -> Result<Amount> {
        let mut index_order = index_order.write();

        if let Some(true) = index_order
            .order_updates
            .front()
            .and_then(|x| Some(x.side.opposite_side() != index_order_update.side))
        {
            if let Some(x) = self.match_index_order_updates(
                &mut index_order,
                index_order_update.remaining_quantity,
            )? {
                index_order_update.remaining_quantity = x;
                index_order.order_updates.push_back(index_order_update);
                Ok(x)
            } else {
                Ok(Amount::ZERO)
            }
        } else {
            let x = index_order_update.remaining_quantity;
            index_order.order_updates.push_back(index_order_update);
            Ok(x)
        }
    }

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
                Arc::new(RwLock::new(IndexOrder {
                    original_address: address.clone(),
                    original_client_order_id: client_order_id.clone(),
                    symbol: symbol.clone(),
                    created_timestamp: timestamp.clone(),
                    last_update_timestamp: timestamp.clone(),
                    order_updates: VecDeque::new(),
                    closed_updates: VecDeque::new(),
                }))
            })
            .clone();

        // Add update to index order
        let quantity_remaining = self.update_index_order(
            index_order,
            IndexOrderUpdate {
                address: address.clone(),
                client_order_id: client_order_id.clone(),
                payment_id: payment_id.clone(),
                side,
                price,
                price_threshold,
                original_quantity: quantity,
                remaining_quantity: quantity,
                update_fee: Amount::ZERO,
                timestamp: timestamp.clone(),
            },
        )?;

        self.observer
            .publish_single(IndexOrderEvent::NewIndexOrder {
                address,
                client_order_id,
                payment_id,
                symbol,
                side,
                price,
                price_threshold,
                quantity_removed: quantity.checked_sub(quantity_remaining).ok_or(eyre!("Math overflow"))?,
                quantity_added: quantity_remaining,
                timestamp,
            });

        Ok(())
    }

    fn cancel_index_order(
        &self,
        address: Address,
        client_order_id: ClientOrderId,
        payment_id: PaymentId,
        symbol: Symbol,
        quantity: Amount,
        timestamp: DateTime<Utc>,
    ) -> Result<()> {
        if let Some(user_index_orders) = self.index_orders.get(&address) {
            if let Some(index_order) = user_index_orders.get(&symbol) {
                let mut index_order = index_order.write();

                if let Some(side_cancelled) =
                    index_order.order_updates.front().and_then(|x| Some(x.side))
                {
                    let (order_cancelled, quantity_cancelled) = if let Some(x) =
                        self.match_index_order_updates(&mut index_order, quantity)?
                    {
                        // complete cancel
                        (true, quantity.checked_sub(x).ok_or(eyre!("Math overflow"))?)
                    } else {
                        // partial cancel
                        (false, quantity)
                    };

                    self.observer
                        .publish_single(IndexOrderEvent::CancelIndexOrder {
                            address,
                            client_order_id,
                            payment_id,
                            symbol,
                            order_cancelled,
                            side_cancelled,
                            quantity_cancelled,
                            timestamp,
                        });
                }
            }
            Ok(())
        } else {
            Err(eyre!("Index order not found"))
        }
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
            } => {
                self.new_index_order(
                    address.clone(),
                    client_order_id.clone(),
                    payment_id.clone(),
                    symbol.clone(),
                    *side,
                    *price,
                    *price_threshold,
                    *quantity,
                    timestamp.clone(),
                )
            }
            ServerEvent::CancelIndexOrder {
                address,
                client_order_id,
                payment_id,
                symbol,
                quantity,
                timestamp,
            } => {
                self.cancel_index_order(
                    address.clone(),
                    client_order_id.clone(),
                    payment_id.clone(),
                    symbol.clone(),
                    *quantity,
                    timestamp.clone(),
                )
            }
            _ => Ok(()),
        }
    }

    /// provide a method to fill index order request
    pub fn fill_order_request(&mut self, _client_order_id: ClientOrderId, _fill_amount: Amount) {
        todo!()
    }

    /// provide a method to list pending index order requests
    pub fn get_pending_order_requests(&self) -> Vec<()> {
        todo!()
    }
}
