use std::{
    collections::{hash_map::Entry, HashMap, HashSet},
    sync::Arc,
};

use chrono::{DateTime, Utc};
use eyre::{eyre, OptionExt, Result};
use itertools::Itertools;
use parking_lot::RwLock;
use safe_math::safe;

use derive_with_baggage::WithBaggage;
use opentelemetry::propagation::Injector;
use serde_json::json;
use symm_core::core::{
    persistence::{Persist, Persistence},
    telemetry::{TracingData, WithBaggage},
};

use crate::{
    collateral::collateral_position::CollateralPosition,
    server::server::{
        CancelIndexOrderNakReason, NewIndexOrderNakReason, Server, ServerError, ServerEvent,
        ServerResponse, ServerResponseReason,
    },
    solver::{
        index_order::IndexOrder, mint_invoice::MintInvoice,
        mint_invoice_manager::MintInvoiceManager,
    },
};
use symm_core::core::{
    bits::{Address, Amount, BatchOrderId, ClientOrderId, PaymentId, Side, Symbol},
    decimal_ext::DecimalExt,
    functional::{IntoObservableSingle, PublishSingle, SingleObserver},
};

use super::{
    index_order::{CancelIndexOrderOutcome, IndexOrderUpdate, UpdateIndexOrderOutcome},
    mint_invoice::{print_fill_report, IndexOrderUpdateReport},
    solver_order::{SolverOrderAssetLot, SolverOrderStatus},
};

pub struct EngageOrderRequest {
    pub chain_id: u32,
    pub address: Address,
    pub client_order_id: ClientOrderId,
    pub symbol: Symbol,
    pub collateral_amount: Amount,
}

pub struct EngagedIndexOrder {
    // Chain ID
    pub chain_id: u32,

    /// On-chain address of the User
    pub address: Address,

    // ID of the NewOrder request
    pub client_order_id: ClientOrderId,

    /// Quantity remaining
    pub collateral_engaged: Amount,

    /// Quantity remaining
    pub collateral_remaining: Amount,
}

#[derive(WithBaggage)]
pub enum IndexOrderEvent {
    NewIndexOrder {
        // Chain ID
        #[baggage]
        chain_id: u32,

        /// On-chain address of the User
        #[baggage]
        address: Address,

        // ID of the NewOrder request
        #[baggage]
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
        #[baggage]
        batch_order_id: BatchOrderId,

        // A set of index orders in the engagement batch
        engaged_orders: HashMap<(Address, ClientOrderId), EngagedIndexOrder>,

        /// Time when requested
        timestamp: DateTime<Utc>,
    },
    CollateralReady {
        // Chain ID
        #[baggage]
        chain_id: u32,

        /// On-chain address of the User
        #[baggage]
        address: Address,

        // ID of the NewOrder request
        #[baggage]
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
        // Chain ID
        #[baggage]
        chain_id: u32,

        /// On-chain address of the User
        #[baggage]
        address: Address,

        // ID of the NewOrder request
        #[baggage]
        client_order_id: ClientOrderId,

        /// Quantity removed
        collateral_removed: Amount,

        /// Quantity remaining
        collateral_remaining: Amount,

        /// Time when requested
        timestamp: DateTime<Utc>,
    },
    CancelIndexOrder {
        // Chain ID
        #[baggage]
        chain_id: u32,

        /// On-chain address of the User
        #[baggage]
        address: Address,

        /// ID of the Cancel request
        #[baggage]
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
    invoice_manager: Arc<RwLock<MintInvoiceManager>>,
    persistence: Arc<dyn Persistence + Send + Sync + 'static>,
    index_orders: HashMap<(u32, Address), HashMap<Symbol, Box<IndexOrder>>>,
    index_symbols: HashSet<Symbol>,
    tolerance: Amount,
}

/// manage index orders, receive orders and route into solver
impl IndexOrderManager {
    pub fn new(
        server: Arc<RwLock<dyn Server>>,
        invoice_manager: Arc<RwLock<MintInvoiceManager>>,
        persistence: Arc<dyn Persistence + Send + Sync + 'static>,
        tolerance: Amount,
    ) -> Self {
        Self {
            observer: SingleObserver::new(),
            server,
            invoice_manager,
            persistence,
            index_orders: HashMap::new(),
            index_symbols: HashSet::new(),
            tolerance,
        }
    }

    pub fn add_index_symbol(&mut self, symbol: Symbol) {
        self.index_symbols.insert(symbol);
    }

    pub fn remove_index_symbol(&mut self, symbol: Symbol) {
        self.index_symbols.remove(&symbol);
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
    ) -> Result<(), ServerResponseReason<NewIndexOrderNakReason>> {
        // Temporary sell side block
        if side == Side::Sell {
            return Err(ServerResponseReason::User(
                NewIndexOrderNakReason::OtherReason {
                    detail: "We don't support Sell yet!".to_string(),
                },
            ));
        }

        // Returns error if basket does not exist
        if !self.index_symbols.contains(&symbol) {
            tracing::warn!(
                %chain_id, %address, %client_order_id, %symbol, "Basket does not exist"
            );
            return Err(ServerResponseReason::User(
                NewIndexOrderNakReason::InvalidSymbol {
                    detail: symbol.to_string(),
                },
            ));
        }

        // Create index orders for user if not created yet
        let user_index_orders = self
            .index_orders
            .entry((chain_id, address))
            .or_insert_with(|| HashMap::new());

        for index_order in user_index_orders.values() {
            if index_order.find_order_update(&client_order_id).is_some() {
                Err(ServerResponseReason::User(
                    NewIndexOrderNakReason::DuplicateClientOrderId {
                        detail: format!("Duplicate client order ID {}", client_order_id),
                    },
                ))?;
            }
        }

        // Create index order if not created yet
        let index_order = user_index_orders.entry(symbol.clone()).or_insert_with(|| {
            Box::new(IndexOrder::new(
                chain_id,
                address.clone(),
                symbol.clone(),
                side,
                timestamp.clone(),
            ))
        });

        // Add update to index order
        let update_order_outcome = index_order
            .update_order(
                client_order_id.clone(),
                side,
                collateral_amount,
                timestamp,
                self.tolerance,
            )
            .map_err(|err| {
                ServerResponseReason::Server(ServerError::OtherReason {
                    detail: format!("Cannot update order: {}", err),
                })
            })?;

        tracing::info!(
            %chain_id,
            %address,
            %client_order_id,
            %symbol,
            orders = %json!(
                index_order.order_updates.iter()
                    .map(|u| (
                        json!(u.client_order_id),
                        u.collateral_spent,
                        u.remaining_collateral,
                        u.filled_quantity))
                    .collect_vec()
            ),
            "User Index Orders"
        );

        match update_order_outcome {
            UpdateIndexOrderOutcome::Push {
                new_collateral_amount,
            } => {
                self.observer
                    .publish_single(IndexOrderEvent::NewIndexOrder {
                        chain_id,
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
                        chain_id,
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
                        chain_id,
                        address,
                        client_order_id: client_order_id.clone(),
                        timestamp,
                    });
                self.observer
                    .publish_single(IndexOrderEvent::NewIndexOrder {
                        chain_id,
                        address,
                        client_order_id: client_order_id.clone(),
                        symbol,
                        side,
                        collateral_amount: new_collateral_amount,
                        timestamp,
                    });
            }
        };
        Ok(())
    }

    /// We would've received CancelOrder request from FIX server.
    fn cancel_index_order(
        &mut self,
        chain_id: u32,
        address: Address,
        client_order_id: ClientOrderId,
        symbol: Symbol,
        collateral_amount: Amount,
        timestamp: DateTime<Utc>,
    ) -> Result<(), ServerResponseReason<CancelIndexOrderNakReason>> {
        let user_orders = self
            .index_orders
            .get_mut(&(chain_id, address))
            .ok_or_else(|| {
                ServerResponseReason::User(CancelIndexOrderNakReason::IndexOrderNotFound {
                    detail: format!("No orders found for user {}", address),
                })
            })?;

        let index_order = user_orders.get_mut(&symbol).ok_or_else(|| {
            ServerResponseReason::User(CancelIndexOrderNakReason::IndexOrderNotFound {
                detail: format!("No order found for user {} for {}", address, symbol),
            })
        })?;

        match index_order
            .cancel_updates(collateral_amount, self.tolerance)
            .map_err(|err| {
                ServerResponseReason::Server(ServerError::OtherReason {
                    detail: format!("Cannot update order: {}", err),
                })
            })? {
            CancelIndexOrderOutcome::Cancel {
                removed_collateral: _,
            } => {
                self.observer
                    .publish_single(IndexOrderEvent::CancelIndexOrder {
                        chain_id,
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
                        chain_id,
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
            } => {
                if let Err(reason) = self.new_index_order(
                    *chain_id,
                    address.clone(),
                    client_order_id.clone(),
                    symbol.clone(),
                    *side,
                    *collateral_amount,
                    timestamp.clone(),
                ) {
                    let result = match &reason {
                        ServerResponseReason::User(..) => Ok(()),
                        ServerResponseReason::Server(err) => {
                            Err(eyre!("Internal server error: {:?}", err))
                        }
                    };
                    self.server
                        .write()
                        .respond_with(ServerResponse::NewIndexOrderNak {
                            chain_id: *chain_id,
                            address: *address,
                            client_order_id: client_order_id.clone(),
                            reason,
                            timestamp: *timestamp,
                        });
                    result
                } else {
                    self.server
                        .write()
                        .respond_with(ServerResponse::NewIndexOrderAck {
                            chain_id: *chain_id,
                            address: *address,
                            client_order_id: client_order_id.clone(),
                            timestamp: *timestamp,
                        });
                    Ok(())
                }
            }
            ServerEvent::CancelIndexOrder {
                chain_id,
                address,
                client_order_id,
                symbol,
                collateral_amount,
                timestamp,
            } => {
                if let Err(reason) = self.cancel_index_order(
                    *chain_id,
                    address.clone(),
                    client_order_id.clone(),
                    symbol.clone(),
                    *collateral_amount,
                    timestamp.clone(),
                ) {
                    let result = match &reason {
                        ServerResponseReason::User(..) => Ok(()),
                        ServerResponseReason::Server(err) => {
                            Err(eyre!("Internal server error: {:?}", err))
                        }
                    };
                    self.server
                        .write()
                        .respond_with(ServerResponse::CancelIndexOrderNak {
                            chain_id: *chain_id,
                            address: *address,
                            client_order_id: client_order_id.clone(),
                            reason,
                            timestamp: *timestamp,
                        });
                    result
                } else {
                    self.server
                        .write()
                        .respond_with(ServerResponse::CancelIndexOrderAck {
                            chain_id: *chain_id,
                            address: *address,
                            client_order_id: client_order_id.clone(),
                            timestamp: *timestamp,
                        });
                    Ok(())
                }
            }
            _ => Ok(()),
        }
    }

    pub fn collateral_ready(
        &mut self,
        chain_id: u32,
        address: &Address,
        client_order_id: &ClientOrderId,
        symbol: &Symbol,
        collateral_amount: Amount,
        fees: Amount,
        timestamp: DateTime<Utc>,
    ) -> Result<()> {
        self.index_orders
            .get_mut(&(chain_id, *address))
            .and_then(|x| x.get_mut(symbol))
            .and_then(|order_write| {
                let (update_remaining_collateral, update_collateral_spent, update_fee) = {
                    let update_write = order_write
                        .order_updates
                        .iter_mut()
                        .find(|update| update.client_order_id.eq(client_order_id))?;

                    let update_upread: &IndexOrderUpdate = update_write;
                    let update_collateral_spent = safe!(update_upread.collateral_spent + fees)?;
                    let update_fee = safe!(update_upread.update_fee + fees)?;
                    let update_remaining_collateral = safe!(update_upread.remaining_collateral - fees)?;

                    tracing::info!(
                            %chain_id, %address, %client_order_id,
                            %update_collateral_spent,
                            %fees,
                            %update_fee,
                            %update_remaining_collateral,
                            %collateral_amount,
                            "Update collateral Ready");

                    if update_remaining_collateral < safe!(collateral_amount - self.tolerance)? {
                        tracing::warn!(
                            %chain_id, %address, %client_order_id,
                            %update_remaining_collateral,
                            %collateral_amount,
                            "Error updating collateral ready: update_remaining_collateral < collateral_amount",
                        );
                        return None;
                    }

                    update_write.collateral_spent = update_collateral_spent;
                    update_write.update_fee = update_fee;
                    update_write.remaining_collateral = update_remaining_collateral;
                    update_write.timestamp = timestamp;

                    Some((update_remaining_collateral, update_collateral_spent, update_fee))
                }?;

                let order_upread: &IndexOrder = order_write;
                let order_remaining_collateral = safe!(order_upread.remaining_collateral - fees)?;
                let order_collateral_spent = safe!(order_upread.collateral_spent + fees)?;
                let order_fees = safe!(order_upread.fees + fees)?;

                order_write.remaining_collateral = order_remaining_collateral;
                order_write.collateral_spent = order_collateral_spent;
                order_write.fees = order_fees;
                order_write.last_update_timestamp = timestamp;

                self.observer
                    .publish_single(IndexOrderEvent::CollateralReady {
                        chain_id,
                        address: *address,
                        client_order_id: client_order_id.clone(),
                        collateral_remaining: update_remaining_collateral,
                        collateral_spent: update_collateral_spent,
                        fees: update_fee,
                        timestamp,
                    });

                self.server
                    .write()
                    .respond_with(ServerResponse::IndexOrderFill {
                        chain_id,
                        address: *address,
                        client_order_id: client_order_id.clone(),
                        filled_quantity: Amount::ZERO,
                        collateral_spent: update_collateral_spent,
                        collateral_remaining: update_remaining_collateral,
                        fill_rate: Amount::ZERO,
                        status: "collateral ready".to_owned(),
                        timestamp,
                    });

                Some(())
            })
            .ok_or_eyre("Failed to update index order")?;
        Ok(())
    }

    fn remove_index_order(
        &mut self,
        chain_id: u32,
        address: &Address,
        symbol: &Symbol,
        client_order_id: &ClientOrderId,
    ) -> Result<Option<Box<IndexOrder>>> {
        match self.index_orders.entry((chain_id, *address)) {
            Entry::Occupied(mut entry) => {
                let result = match entry.get_mut().entry(symbol.clone()) {
                    Entry::Occupied(mut inner_entry) => {
                        let should_remove = (|| -> Result<bool> {
                            let index_order = inner_entry.get_mut();
                            index_order.solver_complete(client_order_id)?;
                            Ok(index_order.order_updates.is_empty())
                        })()?;
                        if should_remove {
                            tracing::info!(
                                %chain_id,
                                %address,
                                %symbol,
                                %client_order_id,
                                "Removing entry: No more updates"
                            );
                            Ok(Some(inner_entry.remove()))
                        } else {
                            Ok(None)
                        }
                    }
                    Entry::Vacant(_) => Err(eyre!(
                        "No Index orders found for: [{}:{}]",
                        chain_id,
                        address
                    )),
                }?;
                if entry.get().is_empty() {
                    tracing::info!(
                        %chain_id,
                        %address,
                        %symbol,
                        %client_order_id,
                        "Removing entry: No more orders",
                    );
                }
                Ok(result)
            }
            Entry::Vacant(_) => Err(eyre!(
                "No Index orders found for: [{}:{}]",
                chain_id,
                address
            )),
        }
    }

    fn find_index_order(
        &self,
        chain_id: u32,
        address: &Address,
        symbol: &Symbol,
    ) -> Option<&IndexOrder> {
        self.index_orders
            .get(&(chain_id, *address))
            .and_then(|x| x.get(symbol))
            .map(|x| x.as_ref())
    }

    fn find_index_order_mut(
        &mut self,
        chain_id: u32,
        address: &Address,
        symbol: &Symbol,
    ) -> Option<&mut IndexOrder> {
        self.index_orders
            .get_mut(&(chain_id, *address))
            .and_then(|x| x.get_mut(symbol))
            .map(|x| x.as_mut())
    }

    /// cleanup after minting
    pub fn order_request_minted(
        &mut self,
        chain_id: u32,
        address: &Address,
        client_order_id: &ClientOrderId,
        symbol: &Symbol,
        payment_id: &PaymentId,
        filled_quantity: Amount,
        amount_paid: Amount,
        lots: Vec<SolverOrderAssetLot>,
        position: CollateralPosition,
        timestamp: DateTime<Utc>,
    ) -> Result<()> {
        let maybe_removed_index_order =
            self.remove_index_order(chain_id, address, symbol, client_order_id)?;

        self.observer
            .publish_single(IndexOrderEvent::CancelIndexOrder {
                chain_id,
                address: *address,
                client_order_id: client_order_id.clone(),
                timestamp,
            });

        let server = self.server.clone();
        let invoice_manager = self.invoice_manager.clone();

        let process = move |index_order: &mut IndexOrder| -> eyre::Result<()> {
            let report = IndexOrderUpdateReport::new(chain_id, *address, symbol.clone());
            let mut update = None;

            index_order.drain_closed_updates(|x| {
                report.report_closed_update(&x);
                if x.client_order_id.eq(client_order_id) {
                    update.replace(x);
                }
            });

            let update = update.ok_or_eyre("Update not found")?;

            let mint_invoice = MintInvoice::try_new(
                index_order,
                &update,
                payment_id,
                filled_quantity,
                amount_paid,
                lots,
                position,
                timestamp,
            )?;

            invoice_manager
                .write()
                .add_invoice(chain_id, *address, mint_invoice.clone())?;

            server.write().respond_with({
                ServerResponse::MintInvoice {
                    chain_id,
                    address: *address,
                    mint_invoice,
                }
            });

            Ok(())
        };

        match maybe_removed_index_order {
            Some(mut index_order) => process(&mut index_order),
            None => {
                let index_order = self
                    .find_index_order_mut(chain_id, address, symbol)
                    .ok_or_eyre("cannot find index order")?;

                process(index_order)
            }
        }
    }

    /// provide a method to fill index order request
    pub fn fill_order_request(
        &mut self,
        chain_id: u32,
        address: &Address,
        client_order_id: &ClientOrderId,
        symbol: &Symbol,
        collateral_spent: Amount,
        fill_amount: Amount,
        fill_rate: Amount,
        status: SolverOrderStatus,
        timestamp: DateTime<Utc>,
    ) -> Result<()> {
        let index_order = self
            .find_index_order(chain_id, address, symbol)
            .ok_or_eyre("cannot find index order")?;

        let update = index_order
            .find_engaged_update(client_order_id)
            .ok_or_eyre("cannot find engaged update")?;

        let (
            update_filled_quantity,
            update_collateral_spent,
            update_engaged_collateral,
            update_collateral_unspent,
            index_order_filled_quantity,
            index_order_collateral_spent,
            index_order_engaged_collateral,
        ) = (|| {
            let index_order_filled_quantity = safe!(index_order.filled_quantity + fill_amount)?;
            let index_order_collateral_spent =
                safe!(index_order.collateral_spent + collateral_spent)?;
            let index_order_engaged_collateral =
                Some(safe!(index_order.engaged_collateral? - collateral_spent)?);

            let update_filled_quantity = safe!(update.filled_quantity + fill_amount)?;
            let update_collateral_spent = safe!(update.collateral_spent + collateral_spent)?;
            let update_engaged_collateral =
                Some(safe!(update.engaged_collateral? - collateral_spent)?);

            let update_collateral_unspent =
                safe!(update_engaged_collateral + update.remaining_collateral)?;

            Some((
                update_filled_quantity,
                update_collateral_spent,
                update_engaged_collateral,
                update_collateral_unspent,
                index_order_filled_quantity,
                index_order_collateral_spent,
                index_order_engaged_collateral,
            ))
        })()
        .ok_or_eyre("Failed to update index order")?;

        print_fill_report(index_order, update, fill_amount, timestamp)?;

        // --==| Commit |==--
        {
            let index_order_mut = self
                .find_index_order_mut(chain_id, address, symbol)
                .ok_or_eyre("cannot find index order")?;

            let update_mut = index_order_mut
                .find_engaged_update_mut(client_order_id)
                .ok_or_eyre("cannot find engaged update")?;

            update_mut.filled_quantity = update_filled_quantity;
            update_mut.collateral_spent = update_collateral_spent;
            update_mut.engaged_collateral = update_engaged_collateral;
            update_mut.timestamp = timestamp;

            index_order_mut.filled_quantity = index_order_filled_quantity;
            index_order_mut.collateral_spent = index_order_collateral_spent;
            index_order_mut.engaged_collateral = index_order_engaged_collateral;
            index_order_mut.last_update_timestamp = timestamp;
        }

        let status_string = match status {
            SolverOrderStatus::Engaged => "in progress",
            SolverOrderStatus::PartlyMintable => "mintable",
            SolverOrderStatus::FullyMintable => "fully mintable",
            _ => "N/A",
        };

        self.server
            .write()
            .respond_with(ServerResponse::IndexOrderFill {
                chain_id,
                address: *address,
                client_order_id: client_order_id.clone(),
                filled_quantity: update_filled_quantity,
                collateral_spent: update_collateral_spent,
                collateral_remaining: update_collateral_unspent,
                fill_rate,
                status: status_string.to_owned(),
                timestamp,
            });

        Ok(())
    }

    pub fn order_failed(
        &mut self,
        chain_id: u32,
        address: &Address,
        client_order_id: &ClientOrderId,
        symbol: &Symbol,
        status: SolverOrderStatus,
        timestamp: DateTime<Utc>,
    ) -> Result<()> {
        let order_failed_span = tracing::info_span!("order-failed",
                %chain_id,
                %address,
                %client_order_id,
                %symbol,
        );
        let _guard = order_failed_span.enter();

        let index_order = self
            .index_orders
            .get_mut(&(chain_id, *address))
            .and_then(|map| map.get_mut(symbol))
            .ok_or_eyre("Cannot find index order")?;

        index_order.solver_cancel(
            client_order_id,
            &format!("Failed with status: {:?}", status),
        );

        tracing::info!(
            remaining_collateral = %index_order.remaining_collateral,
            collateral_spent = %index_order.collateral_spent,
            orders = %json!(
                index_order.order_updates.iter()
                    .map(|u| (
                        json!(u.client_order_id),
                        u.collateral_spent,
                        u.remaining_collateral,
                        u.filled_quantity))
                    .collect_vec()
            ),
            "User Index Orders"
        );

        self.observer
            .publish_single(IndexOrderEvent::CancelIndexOrder {
                chain_id,
                address: *address,
                client_order_id: client_order_id.clone(),
                timestamp,
            });

        self.server
            .write()
            .respond_with(ServerResponse::NewIndexOrderNak {
                chain_id,
                address: *address,
                client_order_id: client_order_id.clone(),
                reason: ServerResponseReason::Server(ServerError::OtherReason {
                    detail: format!(
                        "Cannot handle order: Solver failed with status: {:?}",
                        status
                    ),
                }),
                timestamp,
            });
        Ok(())
    }

    pub fn engage_orders(
        &mut self,
        batch_order_id: BatchOrderId,
        engaged_orders: Vec<EngageOrderRequest>,
    ) -> Result<()> {
        let engage_orders_span = tracing::info_span!("engage-orders", %batch_order_id);
        let _guard = engage_orders_span.enter();

        let mut engage_result = HashMap::new();
        for engage_order in engaged_orders {
            if let Some(index_order) = self
                .index_orders
                .get_mut(&(engage_order.chain_id, engage_order.address))
                .and_then(|map| map.get_mut(&engage_order.symbol))
            {
                tracing::info!(
                    remaining_collateral = %index_order.remaining_collateral,
                    collateral_spent = %index_order.collateral_spent,
                    orders = %json!(
                        index_order.order_updates.iter()
                            .map(|u| (
                                json!(u.client_order_id),
                                u.collateral_spent,
                                u.remaining_collateral,
                                u.filled_quantity))
                            .collect_vec()
                    ),
                    "User Index Orders"
                );

                let (remaining_collateral, unmatched_collateral) = index_order.solver_engage_one(
                    &engage_order.client_order_id,
                    engage_order.collateral_amount,
                    self.tolerance,
                )?;

                tracing::info!(
                    client_order_id = %engage_order.client_order_id,
                    collateral_amount = %engage_order.collateral_amount,
                    engaged_collateral = ?index_order.engaged_collateral,
                    remaining_collateral = %index_order.remaining_collateral,
                    "Engage orders",
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
                            chain_id: engage_order.chain_id,
                            address: engage_order.address,
                            client_order_id: engage_order.client_order_id.clone(),
                            collateral_engaged,
                            collateral_remaining: remaining_collateral,
                        },
                    );
                } else {
                    index_order.solver_cancel(&engage_order.client_order_id, "Math overflow");
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

impl Persist for IndexOrderManager {
    fn load(&mut self) -> Result<()> {
        if let Some(value) = self.persistence.load_value()? {
            if let Some(index_orders_value) = value.get("index_orders") {
                let loaded_orders: HashMap<(u32, Address), HashMap<Symbol, Box<IndexOrder>>> =
                    serde_json::from_value(index_orders_value.clone())
                        .map_err(|err| eyre!("Failed to deserialize index_orders: {:?}", err))?;
                self.index_orders = loaded_orders;
                tracing::info!("Loaded {} index order groups from persistence", self.index_orders.len());
            }

            if let Some(index_symbols_value) = value.get("index_symbols") {
                let loaded_symbols: HashSet<Symbol> =
                    serde_json::from_value(index_symbols_value.clone())
                        .map_err(|err| eyre!("Failed to deserialize index_symbols: {:?}", err))?;
                self.index_symbols = loaded_symbols;
                tracing::info!("Loaded {} index symbols from persistence", self.index_symbols.len());
            }
        }
        Ok(())
    }

    fn store(&self) -> Result<()> {
        let data = json!({
            "index_orders": self.index_orders,
            "index_symbols": self.index_symbols
        });
        self.persistence.store_value(data)
            .map_err(|err| eyre!("Failed to store IndexOrderManager state: {:?}", err))
    }
}

impl IntoObservableSingle<IndexOrderEvent> for IndexOrderManager {
    fn get_single_observer_mut(&mut self) -> &mut SingleObserver<IndexOrderEvent> {
        &mut self.observer
    }
}
