use std::{
    collections::HashMap,
    sync::Arc,
};

use chrono::{DateTime, Utc};
use eyre::{eyre, Result};
use parking_lot::RwLock;

use crate::{
    core::{
        bits::PaymentId,
        functional::{IntoObservableSingle, PublishSingle, SingleObserver},
    },
    server::server::{Server, ServerEvent},
    solver::index_order::IndexOrder,
};

use crate::core::bits::{Address, Amount, ClientOrderId, Side, Symbol};

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
    observer: SingleObserver<IndexOrderEvent>,
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
                Arc::new(RwLock::new(IndexOrder::new(
                    address.clone(),
                    client_order_id.clone(),
                    symbol.clone(),
                    side,
                    timestamp.clone(),
                )))
            })
            .clone();

        // Add update to index order
        let (quantity_removed, quantity_added) = index_order.write().update_order(
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

        self.observer
            .publish_single(IndexOrderEvent::NewIndexOrder {
                address,
                client_order_id: client_order_id.clone(),
                payment_id: payment_id.clone(),
                symbol: symbol.clone(),
                side,
                price,
                price_threshold,
                quantity_removed,
                quantity_added,
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
        let user_orders = self
            .index_orders
            .get(&address)
            .ok_or(eyre!("No orders found for user {}", address))?;
        let index_order = user_orders.get(&symbol).ok_or(eyre!(
            "No order found for user {} for {}",
            address,
            symbol
        ))?;

        match index_order
            .write()
            .cancel_updates(quantity, self.tolerance)?
        {
            (order_cancelled, quantity_cancelled) => {
                self.observer
                    .publish_single(IndexOrderEvent::CancelIndexOrder {
                        address,
                        client_order_id,
                        payment_id,
                        symbol,
                        order_cancelled,
                        side_cancelled: index_order.read().side,
                        quantity_cancelled,
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
    pub fn fill_order_request(&mut self, _client_order_id: ClientOrderId, _fill_amount: Amount) {
        todo!()
    }

    /// provide a method to list pending index order requests
    pub fn get_pending_order_requests(&self) -> Vec<()> {
        todo!()
    }

}

impl IntoObservableSingle<IndexOrderEvent> for IndexOrderManager {
    fn get_single_observer_mut(&mut self) -> &mut SingleObserver<IndexOrderEvent> {
        &mut self.observer
    }
}