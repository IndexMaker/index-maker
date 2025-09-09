use std::{
    collections::{
        self,
        hash_map::{self, Entry},
        HashMap,
    },
    ops::Deref,
    sync::Arc,
};

use arc_swap::{ArcSwap, ArcSwapAny};
use chrono::{DateTime, Utc};
use eyre::{eyre, Context, OptionExt, Result};
use itertools::partition;
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::{
    assets::asset::get_base_asset_symbol_workaround,
    core::{
        functional::{OneShotPublishSingle, OneShotSingleObserver},
        persistence::{Persist, Persistence},
        telemetry::{TracingData, WithBaggage},
    },
    order_sender::{order_tracker::CancelStatus, position::{self, LotAssignment}},
};
use derive_with_baggage::WithBaggage;
use opentelemetry::propagation::Injector;

use crate::{
    core::{
        bits::{Amount, BatchOrder, BatchOrderId, OrderId, Side, SingleOrder, Symbol},
        functional::{IntoObservableSingle, PublishSingle, SingleObserver},
    },
    order_sender::order_tracker::{OrderTracker, OrderTrackerNotification},
};

use super::position::{LotId, Position};

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct GetPositionsResponse {
    pub positions: HashMap<Symbol, Box<Position>>,
    pub missing_symbols: Vec<Symbol>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ReconciledPosition {
    pub inventory_position: Option<Position>,
    pub actual_balance: Option<Amount>,
}

#[derive(Serialize, Deserialize, Default, Clone, Debug)]
pub struct GetReconciledPositionsResponse {
    pub positions: HashMap<Symbol, Box<ReconciledPosition>>,
}

#[derive(Debug, WithBaggage)]
pub enum InventoryEvent {
    OpenLot {
        #[baggage]
        order_id: OrderId,

        #[baggage]
        batch_order_id: BatchOrderId,

        #[baggage]
        lot_id: LotId,

        symbol: Symbol,
        side: Side,
        price: Amount,
        quantity: Amount,
        fee: Amount,
        original_batch_quantity: Amount,
        batch_quantity_remaining: Amount,
        timestamp: DateTime<Utc>,
    },
    CloseLot {
        #[baggage]
        original_order_id: OrderId,

        #[baggage]
        original_batch_order_id: BatchOrderId,

        #[baggage]
        original_lot_id: LotId,

        #[baggage]
        closing_order_id: OrderId,

        #[baggage]
        closing_batch_order_id: BatchOrderId,

        #[baggage]
        closing_lot_id: LotId,

        symbol: Symbol,
        side: Side,
        original_price: Amount,     // original price when lot was opened
        closing_price: Amount,      // price in this closing event
        closing_fee: Amount,        // fee paid for closing event
        quantity_closed: Amount,    // quantity closed in this event
        original_quantity: Amount,  // original quantity when lot was opened
        quantity_remaining: Amount, // quantity remaining in the lot
        closing_batch_original_quantity: Amount,
        closing_batch_quantity_remaining: Amount,
        original_timestamp: DateTime<Utc>,
        closing_timestamp: DateTime<Utc>,
    },
    Cancel {
        #[baggage]
        order_id: OrderId,

        #[baggage]
        batch_order_id: BatchOrderId,

        symbol: Symbol,
        side: Side,
        quantity_cancelled: Amount,
        original_quantity: Amount,
        quantity_remaining: Amount,
        cancel_status: CancelStatus,
        cancel_timestamp: DateTime<Utc>,
    },
}

pub struct InventoryManagerSnapshot {
    positions: ArcSwap<GetReconciledPositionsResponse>,
}

impl InventoryManagerSnapshot {
    pub fn new() -> Self {
        Self {
            positions: ArcSwap::new(Arc::new(GetReconciledPositionsResponse::default())),
        }
    }

    pub fn set(&self, response: Arc<GetReconciledPositionsResponse>) {
        self.positions.store(response);
    }

    pub fn get(&self) -> Arc<GetReconciledPositionsResponse> {
        self.positions.load_full()
    }
}

pub struct InventoryManager {
    observer: SingleObserver<InventoryEvent>,
    order_tracker: Arc<RwLock<OrderTracker>>,
    persistence: Arc<dyn Persistence + Send + Sync + 'static>,
    positions: HashMap<Symbol, Box<Position>>,
    snapshot: Arc<InventoryManagerSnapshot>,
    tolerance: Amount,
}

impl InventoryManager {
    pub fn new(
        order_tracker: Arc<RwLock<OrderTracker>>,
        persistence: Arc<dyn Persistence + Send + Sync + 'static>,
        tolerance: Amount,
    ) -> Self {
        Self {
            observer: SingleObserver::new(),
            persistence,
            order_tracker,
            positions: HashMap::new(),
            snapshot: Arc::new(InventoryManagerSnapshot::new()),
            tolerance,
        }
    }

    /// notify about new lots to subscriber (-> Solver)
    pub fn notify_allocated_lots(&self) {}

    fn create_lot(
        &mut self,
        order_id: OrderId,
        batch_order_id: BatchOrderId,
        lot_id: LotId,
        symbol: Symbol,
        side: Side,
        price_filled: Amount,
        quantity_filled: Amount,
        fee_paid: Amount,
        original_quantity: Amount,
        quantity_remaining: Amount,
        fill_timestamp: DateTime<Utc>,
    ) -> Result<()> {
        let base_asset = get_base_asset_symbol_workaround(&symbol);
        let position = self
            .positions
            .entry(get_base_asset_symbol_workaround(&base_asset))
            .or_insert_with(|| Box::new(Position::new(base_asset.clone(), side)));

        position.create_lot(
            order_id.clone(),
            batch_order_id.clone(),
            lot_id.clone(),
            side,
            price_filled,
            quantity_filled,
            fee_paid,
            fill_timestamp.clone(),
        )?;

        self.observer.publish_single(InventoryEvent::OpenLot {
            order_id,
            batch_order_id,
            lot_id,
            symbol: symbol.clone(),
            side,
            price: price_filled,
            quantity: quantity_filled,
            fee: fee_paid,
            original_batch_quantity: original_quantity,
            batch_quantity_remaining: quantity_remaining,
            timestamp: fill_timestamp,
        });

        Ok(())
    }

    /// Match fill against remaining quantity within currently open lots.
    /// Returns any unmatched quantity.
    fn match_lots(
        &mut self,
        order_id: OrderId,
        batch_order_id: BatchOrderId,
        lot_id: LotId,
        symbol: Symbol,
        side: Side,
        price_filled: Amount,
        quantity_filled: Amount,
        fee_paid: Amount,
        batch_original_quantity: Amount,
        batch_quantity_remaining: Amount,
        fill_timestamp: DateTime<Utc>,
    ) -> Result<Option<Amount>> {
        let base_asset = get_base_asset_symbol_workaround(&symbol);
        // Find position for symbol
        match self.positions.get_mut(&base_asset) {
            // Position not found
            None => Ok(Some(quantity_filled)),
            Some(position) => {
                // Match lots
                let remaining = position.match_lots(
                    order_id.clone(),
                    batch_order_id.clone(),
                    lot_id.clone(),
                    side,
                    price_filled,
                    quantity_filled,
                    fee_paid,
                    fill_timestamp.clone(),
                    self.tolerance,
                )?;

                position.drain_closed_lots_and_callback_on_updated(|lot| {
                    self.observer.publish_single(InventoryEvent::CloseLot {
                        original_order_id: lot.original_order_id.clone(),
                        original_batch_order_id: lot.original_batch_order_id.clone(),
                        original_lot_id: lot.lot_id.clone(),
                        closing_order_id: order_id.clone(),
                        closing_batch_order_id: batch_order_id.clone(),
                        closing_lot_id: lot_id.clone(),
                        symbol: symbol.clone(),
                        side: side.opposite_side(),
                        original_price: lot.original_price,
                        closing_price: price_filled,
                        closing_fee: fee_paid,
                        quantity_closed: lot.get_last_transaction_quantity(),
                        original_quantity: lot.original_quantity,
                        quantity_remaining: lot.remaining_quantity,
                        original_timestamp: lot.created_timestamp,
                        closing_batch_original_quantity: batch_original_quantity,
                        closing_batch_quantity_remaining: batch_quantity_remaining,
                        closing_timestamp: fill_timestamp,
                    });
                });

                Ok(remaining)
            }
        }
    }

    /// receive fill reports from Order Tracker
    pub fn handle_fill_report(&mut self, notification: OrderTrackerNotification) -> Result<()> {
        // 1. match against lots (in case of Sell), P&L report
        // 2. allocate new lots, store Cost/Fees
        //self.notify_lots(&[Lot::default()]);
        match notification {
            OrderTrackerNotification::Fill {
                order_id,
                batch_order_id,
                lot_id,
                symbol,
                side,
                price_filled,
                quantity_filled,
                fee_paid,
                original_quantity,
                quantity_remaining,
                is_cancelled,
                fill_timestamp,
            } => {
                // match against open lots, close lots
                // send CloseLot event to subscriber (-> Solver)
                if let Some(unmatched_quantity) = self.match_lots(
                    order_id.clone(),
                    batch_order_id.clone(),
                    lot_id.clone(),
                    symbol.clone(),
                    side,
                    price_filled,
                    quantity_filled,
                    fee_paid,
                    original_quantity,
                    quantity_remaining,
                    fill_timestamp,
                )? {
                    // open new lot
                    // send OpenLot event to subscriber (-> Solver)
                    self.create_lot(
                        order_id.clone(),
                        batch_order_id.clone(),
                        lot_id,
                        symbol.clone(),
                        side,
                        price_filled,
                        unmatched_quantity,
                        fee_paid,
                        original_quantity,
                        quantity_remaining,
                        fill_timestamp,
                    )?;
                }
                if is_cancelled {
                    self.observer.publish_single(InventoryEvent::Cancel {
                        order_id,
                        batch_order_id,
                        symbol,
                        side,
                        quantity_cancelled: Amount::ZERO,
                        original_quantity,
                        quantity_remaining,
                        cancel_status: CancelStatus::FullyCancelled,
                        cancel_timestamp: fill_timestamp,
                    });
                }
                Ok(())
            }

            OrderTrackerNotification::Cancel {
                order_id,
                batch_order_id,
                symbol,
                side,
                quantity_cancelled,
                original_quantity,
                quantity_remaining,
                cancel_status,
                cancel_timestamp,
            } => {
                self.observer.publish_single(InventoryEvent::Cancel {
                    order_id,
                    batch_order_id,
                    symbol,
                    side,
                    quantity_cancelled,
                    original_quantity,
                    quantity_remaining,
                    cancel_status,
                    cancel_timestamp,
                });
                Ok(())
            }
        }
    }

    /// receive new order requests from Solver
    pub fn new_order_batch(&self, batch_order: Arc<BatchOrder>) -> Result<()> {
        // Start writing to Order Tracker
        let mut guard = self.order_tracker.write();
        // Send all orders out
        for asset_order in &batch_order.asset_orders {
            tracing::debug!(
                order_id = %asset_order.order_id,
                symbol = %asset_order.symbol,
                side = ?asset_order.side,
                price = %asset_order.price,
                quantity = %asset_order.quantity,
                "Sending order");
            guard
                .new_order(Arc::new(SingleOrder {
                    order_id: asset_order.order_id.clone(),
                    batch_order_id: batch_order.batch_order_id.clone(),
                    symbol: asset_order.symbol.clone(),
                    side: asset_order.side,
                    price: asset_order.price,
                    quantity: asset_order.quantity,
                    created_timestamp: batch_order.created_timestamp,
                }))
                .map_err(|err| {
                    eyre!(
                        "Failed to create new order for {} in basket {}: {:?}",
                        asset_order.symbol,
                        batch_order.batch_order_id,
                        err
                    )
                })?;
        }
        // All orders out
        Ok(())
    }

    pub fn assign_lots(
        &mut self,
        lot_assignments: HashMap<(Symbol, LotId), Vec<Arc<LotAssignment>>>,
    ) -> Result<()> {
        let nested_map =
            lot_assignments
                .into_iter()
                .fold(HashMap::new(), |mut map, ((symbol, lot_id), v)| {
                    match map.entry(symbol) {
                        Entry::Vacant(vacant_entry) => {
                            let x = HashMap::from([(lot_id, v)]);
                            vacant_entry.insert(x);
                        }
                        Entry::Occupied(mut occupied_entry) => {
                            let x = occupied_entry.get_mut();
                            match x.entry(lot_id) {
                                Entry::Occupied(mut occupied_entry) => {
                                    occupied_entry.get_mut().extend(v);
                                }
                                Entry::Vacant(vacant_entry) => {
                                    vacant_entry.insert(v);
                                }
                            }
                        }
                    }
                    map
                });

        for (symbol, map) in nested_map {
            let position = self
                .positions
                .get_mut(&symbol)
                .ok_or_eyre("Failed to find position")?;

            position.assign_lots(map)?;
        }

        Ok(())
    }

    /// provide method to get open lots
    pub fn get_positions(&self, symbols: &[Symbol]) -> GetPositionsResponse {
        // Optimistic: we should be able to find all symbols
        let mut positions = HashMap::with_capacity(symbols.len());

        // ...but first we copy all symbols into a Vec
        let mut missing_symbols = symbols.to_vec();

        // ...and then we should collect open lots, and find the missing ones
        let partition_point = partition(&mut missing_symbols, |symbol| {
            if let Some(position) = self.positions.get(symbol) {
                positions.insert(symbol.clone(), position.clone());
                false
            } else {
                true
            }
        });

        missing_symbols.splice(partition_point.., []);

        GetPositionsResponse {
            positions,
            missing_symbols,
        }
    }

    pub fn get_reconciled_positions(
        &self,
        observer: OneShotSingleObserver<GetReconciledPositionsResponse>,
    ) -> eyre::Result<()> {
        let mut reconciled_positions = GetReconciledPositionsResponse {
            positions: self
                .positions
                .iter()
                .map(|(symbol, position)| {
                    (
                        symbol.clone(),
                        Box::new(ReconciledPosition {
                            inventory_position: Some(position.as_ref().clone()),
                            actual_balance: Some(Amount::ZERO),
                        }),
                    )
                })
                .collect(),
        };

        self.order_tracker
            .read()
            .get_balances(OneShotSingleObserver::new_with_fn(
                move |balances: HashMap<Symbol, Amount>| {
                    balances.into_iter().for_each(|(symbol, balance)| {
                        match reconciled_positions.positions.entry(symbol) {
                            Entry::Occupied(mut occupied_entry) => {
                                occupied_entry.get_mut().actual_balance.replace(balance);
                            }
                            Entry::Vacant(vacant_entry) => {
                                vacant_entry.insert(Box::new(ReconciledPosition {
                                    inventory_position: None,
                                    actual_balance: Some(balance),
                                }));
                            }
                        };
                    });
                    observer.one_shot_publish_single(reconciled_positions);
                },
            ))?;
        Ok(())
    }
    
    pub fn update_snapshot(&self) -> Result<()> {
        let snapshot = self.snapshot.clone();

        self.get_reconciled_positions(OneShotSingleObserver::new_with_fn(move |x| {
            snapshot.set(Arc::new(x))
        }))
    }

    pub fn get_snapshot(&self) -> Arc<InventoryManagerSnapshot> {
        self.snapshot.clone()
    }

}

impl Persist for InventoryManager {
    fn load(&mut self) -> Result<()> {
        if let Some(value) = self.persistence.load_value()? {
            if let Some(positions_value) = value.get("positions") {
                let loaded_positions: HashMap<Symbol, Box<Position>> =
                    serde_json::from_value(positions_value.clone())
                        .map_err(|err| eyre!("Failed to deserialize positions: {:?}", err))?;
                self.positions = loaded_positions;
                tracing::info!(
                    "Loaded {} inventory positions from persistence",
                    self.positions.len()
                );
            }
        }
        Ok(())
    }

    fn store(&self) -> Result<()> {
        let data = json!({
            "positions": self.positions
        });
        self.persistence
            .store_value(data)
            .map_err(|err| eyre!("Failed to store InventoryManager state: {:?}", err))
    }
}

impl IntoObservableSingle<InventoryEvent> for InventoryManager {
    fn get_single_observer_mut(&mut self) -> &mut SingleObserver<InventoryEvent> {
        &mut self.observer
    }
}

#[cfg(test)]
mod test {
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };

    use chrono::Utc;
    use itertools::Itertools;
    use parking_lot::RwLock;
    use rust_decimal::dec;

    use crate::{
        assert_decimal_approx_eq,
        core::{
            bits::{Amount, AssetOrder, BatchOrder, BatchOrderId, OrderId, Side, SingleOrder},
            functional::IntoObservableSingle,
            persistence::util::InMemoryPersistence,
            test_util::{get_mock_asset_name_1, get_mock_defer_channel, run_mock_deferred},
        },
        order_sender::{
            order_connector::{
                test_util::MockOrderConnector, OrderConnectorNotification, SessionId,
            },
            order_tracker::{OrderTracker, OrderTrackerNotification},
        },
    };

    use super::{InventoryEvent, InventoryManager, LotId};

    #[test]
    /// Test that InventoryManager is sane.
    ///
    /// First we send Buy order:
    /// * Buy  50.0 A @ $100.0
    ///
    /// And we get two fills on it:
    /// * Fill 10.0 A @ $99.0
    /// * Fill 20.0 A @ $100.0
    ///
    /// The unfilled qty on that order is = 20.0 A
    ///
    /// This creates two lots:
    /// * Lot01 => 10.0 A @ $99.0
    /// * Lot02 => 20.0 A @ $100.0
    ///
    /// Then I send second order:
    /// * Sell 40.0 A @ $120.0
    ///
    /// And we get one fill:
    /// * Fill 15.0 A @ 125.0
    ///
    /// And unfilled quantity on that sell is = 25.0
    ///
    /// This matches two lots:
    ///
    /// * Lot01 => 10.0 A @ $99.0 against 10.0 A @ $125.0 => closes Lot01
    /// * Lot02 => 20.0 A @ $100.0 against 5.0 A @ $125.0 => quantity remaining on Lot02  is = 15.0 A
    ///
    fn test_inventory_manager() {
        let tolerance = dec!(0.000001);

        let (defer_1, deferred) = get_mock_defer_channel();
        let defer_2 = defer_1.clone();
        let defer_3 = defer_1.clone();
        let defer_4 = defer_1.clone();
        let defer_5 = defer_1.clone();
        let defer_6 = defer_1.clone();

        let counter_1 = Arc::new(AtomicUsize::new(0));
        let counter_2 = counter_1.clone();

        let logon_timestamp = Utc::now();

        let order_connector = Arc::new(RwLock::new(MockOrderConnector::new()));
        let order_tracker = Arc::new(RwLock::new(OrderTracker::new(
            order_connector.clone(),
            tolerance,
        )));
        let persistence = Arc::new(InMemoryPersistence::new());
        let inventory_manager = Arc::new(RwLock::new(InventoryManager::new(
            order_tracker.clone(),
            persistence,
            tolerance,
        )));

        let buy_timestamp = Utc::now();

        let buy_order_id: OrderId = "Order01".into();
        let buy_batch_order_id: BatchOrderId = "Batch01".into();
        let buy_lot1_id: LotId = "Lot01".into();
        let buy_lot2_id: LotId = "Lot02".into();

        let buy_order_price = dec!(100.0);
        let buy_order_quantity = dec!(50.0);
        let buy_fill1_price = dec!(99.0);
        let buy_fill2_price = dec!(100.0);
        let buy_fill1_quantity = dec!(10.0);
        let buy_fill2_quantity = dec!(20.0);
        let buy_fee1 = dec!(0.0099);
        let buy_fee2 = dec!(0.0100);

        let sell_timestamp = Utc::now();

        let sell_order_id: OrderId = "Order02".into();
        let sell_batch_order_id: BatchOrderId = "Batch02".into();
        let sell_lot1_id: LotId = "Lot03".into();

        let sell_order_price = dec!(120.0);
        let sell_order_quantity = dec!(40.0);
        let sell_fill1_price = dec!(125.0);
        let sell_fill1_quantity = dec!(15.0);
        let sell_fee1 = dec!(0.0125);

        let mut closed_lot_id = None;
        assert!(matches!(closed_lot_id, None));

        let logout_timestamp = Utc::now();

        // Make sure Order Connector sends events to Order Tracker
        let order_tracker_1 = Arc::downgrade(&order_tracker);
        order_connector
            .write()
            .get_single_observer_mut()
            .set_observer_fn(move |e: OrderConnectorNotification| {
                let order_tracker = order_tracker_1.upgrade().unwrap();
                defer_1
                    .send(Box::new(move || {
                        order_tracker.write().handle_order_notification(e).unwrap();
                    }))
                    .expect("Failed to defer");
            });

        order_connector.write().connect();
        order_connector
            .write()
            .notify_logon("Session-01".into(), logon_timestamp);
        run_mock_deferred(&deferred);

        //
        // Part I. Let's open some lots!
        //
        {
            // Make sure Order Tracker sends events to Inventory Manager
            let inventory_manager_1 = Arc::downgrade(&inventory_manager);
            order_tracker
                .write()
                .get_single_observer_mut()
                .set_observer_fn(move |e: OrderTrackerNotification| {
                    let inventory_manager = inventory_manager_1.upgrade().unwrap();
                    defer_2
                        .send(Box::new(move || {
                            inventory_manager
                                .write()
                                .handle_fill_report(e)
                                .expect("Error handling fill report");
                        }))
                        .expect("Failed to defer");
                });

            // Implement Mock Connector to reply with 2 x Fills
            let order_connector_1 = Arc::downgrade(&order_connector);
            let lot1_id_1 = buy_lot1_id.clone();
            let lot2_id_1 = buy_lot2_id.clone();
            let timestamp_1 = buy_timestamp.clone();
            order_connector.write().implementor.set_observer_fn(
                move |(sid, e): (SessionId, Arc<SingleOrder>)| {
                    let lot1_id = lot1_id_1.clone();
                    let lot2_id = lot2_id_1.clone();
                    let timestamp = timestamp_1.clone();
                    let order_connector = order_connector_1.upgrade().unwrap();
                    defer_3
                        .send(Box::new(move || {
                            order_connector.read().nofity_new(
                                e.order_id.clone(),
                                get_mock_asset_name_1(),
                                Side::Buy,
                                e.price,
                                e.quantity,
                                timestamp,
                            );
                            order_connector.read().notify_fill(
                                e.order_id.clone(),
                                lot1_id,
                                get_mock_asset_name_1(),
                                Side::Buy,
                                buy_fill1_price,
                                buy_fill1_quantity,
                                buy_fee1,
                                timestamp,
                            );
                            order_connector.read().notify_fill(
                                e.order_id.clone(),
                                lot2_id,
                                get_mock_asset_name_1(),
                                Side::Buy,
                                buy_fill2_price,
                                buy_fill2_quantity,
                                buy_fee2,
                                timestamp,
                            );
                        }))
                        .expect("Failed to defer")
                },
            );

            let order_id_2 = buy_order_id.clone();
            let batch_order_id_2 = buy_batch_order_id.clone();
            let lot1_id_2 = buy_lot1_id.clone();
            let lot2_id_2 = buy_lot2_id.clone();
            let timestamp_2 = buy_timestamp.clone();
            inventory_manager
                .write()
                .observer
                .set_observer_fn(move |e: InventoryEvent| {
                    let order_id_2 = order_id_2.clone();
                    let batch_order_id_2 = batch_order_id_2.clone();
                    let lot1_id_2 = lot1_id_2.clone();
                    let lot2_id_2 = lot2_id_2.clone();
                    let timestamp_2 = timestamp_2.clone();
                    let counter = counter_2.clone();
                    defer_4
                        .send(Box::new(move || {
                            match (counter.fetch_add(1, Ordering::Relaxed), e) {
                                (
                                    0,
                                    InventoryEvent::OpenLot {
                                        order_id,
                                        batch_order_id,
                                        lot_id,
                                        symbol,
                                        side,
                                        price,
                                        quantity,
                                        fee,
                                        original_batch_quantity,
                                        batch_quantity_remaining,
                                        timestamp,
                                    },
                                ) => {
                                    assert_eq!(order_id, order_id_2);
                                    assert_eq!(batch_order_id, batch_order_id_2);
                                    assert_eq!(lot_id, lot1_id_2);
                                    assert_eq!(symbol, get_mock_asset_name_1());
                                    assert!(matches!(side, Side::Buy));
                                    assert_decimal_approx_eq!(price, buy_fill1_price, tolerance);
                                    assert_decimal_approx_eq!(
                                        quantity,
                                        buy_fill1_quantity,
                                        tolerance
                                    );
                                    assert_decimal_approx_eq!(fee, buy_fee1, tolerance);
                                    assert_decimal_approx_eq!(
                                        original_batch_quantity,
                                        buy_order_quantity,
                                        tolerance
                                    );
                                    assert_decimal_approx_eq!(
                                        batch_quantity_remaining,
                                        buy_order_quantity.checked_sub(buy_fill1_quantity).unwrap(),
                                        tolerance
                                    );
                                    assert_eq!(timestamp, timestamp_2);
                                }
                                (
                                    1,
                                    InventoryEvent::OpenLot {
                                        order_id,
                                        batch_order_id,
                                        lot_id,
                                        symbol,
                                        side,
                                        price,
                                        quantity,
                                        fee,
                                        original_batch_quantity,
                                        batch_quantity_remaining,
                                        timestamp,
                                    },
                                ) => {
                                    assert_eq!(order_id, order_id_2);
                                    assert_eq!(batch_order_id, batch_order_id_2);
                                    assert_eq!(lot_id, lot2_id_2);
                                    assert_eq!(symbol, get_mock_asset_name_1());
                                    assert!(matches!(side, Side::Buy));
                                    assert_decimal_approx_eq!(price, buy_fill2_price, tolerance);
                                    assert_decimal_approx_eq!(
                                        quantity,
                                        buy_fill2_quantity,
                                        tolerance
                                    );
                                    assert_decimal_approx_eq!(fee, buy_fee2, tolerance);
                                    assert_decimal_approx_eq!(
                                        original_batch_quantity,
                                        buy_order_quantity,
                                        tolerance
                                    );
                                    assert_decimal_approx_eq!(
                                        batch_quantity_remaining,
                                        buy_order_quantity
                                            .checked_sub(buy_fill1_quantity)
                                            .and_then(|x| x.checked_sub(buy_fill2_quantity))
                                            .unwrap(),
                                        tolerance
                                    );
                                    assert_eq!(timestamp, timestamp_2);
                                }
                                _ => panic!("Unexpected count"),
                            }
                        }))
                        .expect("Failed to defer");
                });

            // Let's send simple batch order to buy single asset
            inventory_manager
                .write()
                .new_order_batch(Arc::new(BatchOrder {
                    batch_order_id: buy_batch_order_id.clone(),
                    created_timestamp: buy_timestamp.clone(),
                    asset_orders: vec![AssetOrder {
                        order_id: buy_order_id.clone(),
                        symbol: get_mock_asset_name_1(),
                        side: Side::Buy,
                        price: buy_order_price,
                        quantity: buy_order_quantity,
                    }],
                }))
                .expect("Failed to send order");

            run_mock_deferred(&deferred);

            // We should have received two InventoryEvents
            assert_eq!(counter_1.load(Ordering::Relaxed), 2);

            let all_positions = inventory_manager
                .read()
                .get_positions(&[get_mock_asset_name_1()]);
            assert_eq!(all_positions.missing_symbols.len(), 0);

            let position = all_positions
                .positions
                .get(&get_mock_asset_name_1())
                .unwrap();

            assert_eq!(position.balance, dec!(30.0));

            let lots = &position.open_lots;
            assert_eq!(lots.len(), 2);

            let lot = lots.get(0).unwrap();
            assert_eq!(lot.lot_id, buy_lot1_id);
            assert_eq!(lot.lot_transactions.len(), 0);

            closed_lot_id = Some(lot.lot_id.clone());

            // We will close that lot in next part II.

            let lot = lots.get(1).unwrap();
            assert_eq!(lot.lot_id, buy_lot2_id);
            assert_eq!(lot.lot_transactions.len(), 0);
        }

        //
        // Part II. Let's close some lots!
        //
        {
            // Implement Mock Connector to reply with 1 x Fill
            let order_connector_1 = Arc::downgrade(&order_connector);
            let lot1_id_1 = sell_lot1_id.clone();
            let timestamp_1 = sell_timestamp.clone();
            order_connector.write().implementor.set_observer_fn(
                move |(sid, e): (SessionId, Arc<SingleOrder>)| {
                    let lot1_id = lot1_id_1.clone();
                    let timestamp = timestamp_1.clone();
                    let order_connector = order_connector_1.upgrade().unwrap();
                    defer_5
                        .send(Box::new(move || {
                            order_connector.read().nofity_new(
                                e.order_id.clone(),
                                get_mock_asset_name_1(),
                                Side::Sell,
                                e.price,
                                e.quantity,
                                timestamp,
                            );
                            order_connector.read().notify_fill(
                                e.order_id.clone(),
                                lot1_id,
                                get_mock_asset_name_1(),
                                Side::Sell,
                                sell_fill1_price,
                                sell_fill1_quantity,
                                sell_fee1,
                                timestamp,
                            );
                        }))
                        .expect("Failed to defer")
                },
            );

            let buy_order_id_1 = buy_order_id.clone();
            let buy_batch_order_id_1 = buy_batch_order_id.clone();
            let buy_lot1_id_1 = buy_lot1_id.clone();
            let buy_lot2_id_1 = buy_lot2_id.clone();
            let buy_timestamp_1 = buy_timestamp.clone();
            let sell_order_id_1 = sell_order_id.clone();
            let sell_batch_order_id_1 = sell_batch_order_id.clone();
            let sell_lot1_id_1 = sell_lot1_id.clone();
            let sell_timestamp_1 = sell_timestamp.clone();
            let counter_2 = counter_1.clone();
            inventory_manager
                .write()
                .observer
                .set_observer_fn(move |e: InventoryEvent| {
                    let buy_order_id_1 = buy_order_id_1.clone();
                    let buy_batch_order_id_1 = buy_batch_order_id_1.clone();
                    let buy_lot1_id_1 = buy_lot1_id_1.clone();
                    let buy_lot2_id_1 = buy_lot2_id_1.clone();
                    let buy_timestamp_1 = buy_timestamp_1.clone();
                    let sell_order_id_1 = sell_order_id_1.clone();
                    let sell_batch_order_id_1 = sell_batch_order_id_1.clone();
                    let sell_lot1_id_1 = sell_lot1_id_1.clone();
                    let sell_timestamp_1 = sell_timestamp_1.clone();
                    let counter = counter_2.clone();
                    defer_6
                        .send(Box::new(move || {
                            match (counter.fetch_add(1, Ordering::Relaxed), e) {
                                (
                                    2,
                                    InventoryEvent::CloseLot {
                                        original_order_id,
                                        original_batch_order_id,
                                        original_lot_id,
                                        closing_order_id,
                                        closing_batch_order_id,
                                        closing_lot_id,
                                        symbol,
                                        side,
                                        original_price,
                                        closing_price,
                                        closing_fee,
                                        quantity_closed,
                                        original_quantity,
                                        quantity_remaining,
                                        closing_batch_original_quantity,
                                        closing_batch_quantity_remaining,
                                        original_timestamp,
                                        closing_timestamp,
                                    },
                                ) => {
                                    assert_eq!(original_order_id, buy_order_id_1);
                                    assert_eq!(original_batch_order_id, buy_batch_order_id_1);
                                    assert_eq!(original_lot_id, buy_lot1_id_1);

                                    assert_eq!(closing_order_id, sell_order_id_1);
                                    assert_eq!(closing_batch_order_id, sell_batch_order_id_1);
                                    assert_eq!(closing_lot_id, sell_lot1_id_1);

                                    assert_eq!(symbol, get_mock_asset_name_1());
                                    assert!(matches!(side, Side::Buy));

                                    assert_decimal_approx_eq!(
                                        original_price,
                                        buy_fill1_price,
                                        tolerance
                                    );
                                    assert_decimal_approx_eq!(
                                        closing_price,
                                        sell_fill1_price,
                                        tolerance
                                    );

                                    assert_decimal_approx_eq!(closing_fee, sell_fee1, tolerance);

                                    assert_decimal_approx_eq!(
                                        quantity_closed,
                                        buy_fill1_quantity,
                                        tolerance
                                    );
                                    assert_decimal_approx_eq!(
                                        original_quantity,
                                        buy_fill1_quantity,
                                        tolerance
                                    );
                                    assert_decimal_approx_eq!(
                                        quantity_remaining,
                                        Amount::ZERO,
                                        tolerance
                                    );

                                    assert_decimal_approx_eq!(
                                        closing_batch_original_quantity,
                                        sell_order_quantity,
                                        tolerance
                                    );

                                    assert_decimal_approx_eq!(
                                        closing_batch_quantity_remaining,
                                        sell_order_quantity
                                            .checked_sub(sell_fill1_quantity)
                                            .unwrap(),
                                        tolerance
                                    );

                                    assert_eq!(original_timestamp, buy_timestamp_1);
                                    assert_eq!(closing_timestamp, sell_timestamp_1);
                                }
                                (
                                    3,
                                    InventoryEvent::CloseLot {
                                        original_order_id,
                                        original_batch_order_id,
                                        original_lot_id,
                                        closing_order_id,
                                        closing_batch_order_id,
                                        closing_lot_id,
                                        symbol,
                                        side,
                                        original_price,
                                        closing_price,
                                        closing_fee,
                                        quantity_closed,
                                        original_quantity,
                                        quantity_remaining,
                                        closing_batch_original_quantity,
                                        closing_batch_quantity_remaining,
                                        original_timestamp,
                                        closing_timestamp,
                                    },
                                ) => {
                                    assert_eq!(original_order_id, buy_order_id_1);
                                    assert_eq!(original_batch_order_id, buy_batch_order_id_1);
                                    assert_eq!(original_lot_id, buy_lot2_id_1);

                                    assert_eq!(closing_order_id, sell_order_id_1);
                                    assert_eq!(closing_batch_order_id, sell_batch_order_id_1);
                                    assert_eq!(closing_lot_id, sell_lot1_id_1);

                                    assert_eq!(symbol, get_mock_asset_name_1());
                                    assert!(matches!(side, Side::Buy));

                                    assert_decimal_approx_eq!(
                                        original_price,
                                        buy_fill2_price,
                                        tolerance
                                    );
                                    assert_decimal_approx_eq!(
                                        closing_price,
                                        sell_fill1_price,
                                        tolerance
                                    );

                                    assert_decimal_approx_eq!(closing_fee, sell_fee1, tolerance);

                                    assert_decimal_approx_eq!(
                                        quantity_closed,
                                        sell_fill1_quantity
                                            .checked_sub(buy_fill1_quantity)
                                            .unwrap(),
                                        tolerance
                                    );
                                    assert_decimal_approx_eq!(
                                        original_quantity,
                                        buy_fill2_quantity,
                                        tolerance
                                    );
                                    assert_decimal_approx_eq!(
                                        quantity_remaining,
                                        sell_fill1_quantity
                                            .checked_sub(buy_fill1_quantity)
                                            .and_then(|x| buy_fill2_quantity.checked_sub(x))
                                            .unwrap(),
                                        tolerance
                                    );

                                    assert_decimal_approx_eq!(
                                        closing_batch_original_quantity,
                                        sell_order_quantity,
                                        tolerance
                                    );

                                    assert_decimal_approx_eq!(
                                        closing_batch_quantity_remaining,
                                        sell_order_quantity
                                            .checked_sub(sell_fill1_quantity)
                                            .unwrap(),
                                        tolerance
                                    );

                                    assert_eq!(original_timestamp, buy_timestamp_1);
                                    assert_eq!(closing_timestamp, sell_timestamp_1);
                                }
                                _ => panic!("Unexpected count"),
                            }
                        }))
                        .expect("Failed to defer");
                });

            // Let's send simple batch order to buy single asset
            inventory_manager
                .write()
                .new_order_batch(Arc::new(BatchOrder {
                    batch_order_id: sell_batch_order_id,
                    created_timestamp: sell_timestamp,
                    asset_orders: vec![AssetOrder {
                        order_id: sell_order_id,
                        symbol: get_mock_asset_name_1(),
                        side: Side::Sell,
                        price: sell_order_price,
                        quantity: sell_order_quantity,
                    }],
                }))
                .expect("Failed to send order");

            run_mock_deferred(&deferred);

            // We should have received two InventoryEvents
            assert_eq!(counter_1.load(Ordering::Relaxed), 4);

            let all_positions = inventory_manager
                .read()
                .get_positions(&[get_mock_asset_name_1()]);
            assert_eq!(all_positions.missing_symbols.len(), 0);

            let position = all_positions
                .positions
                .get(&get_mock_asset_name_1())
                .unwrap();

            assert_eq!(position.balance, dec!(15.0));

            let lots = &position.open_lots;
            assert_eq!(lots.len(), 1);

            let lot = lots.front().unwrap();
            assert_eq!(lot.lot_id, buy_lot2_id);

            assert_eq!(lot.lot_transactions.len(), 1);
            let lot_tx = lot.lot_transactions.first().unwrap();
            assert_eq!(lot_tx.matched_lot_id, sell_lot1_id);

            assert!(matches!(closed_lot_id, Some(_)));
            let closed_lot_id = closed_lot_id.unwrap();

            assert!(matches!(
                position
                    .open_lots
                    .iter()
                    .find(|x| x.lot_id.eq(&closed_lot_id)),
                None
            ));

            assert!(matches!(
                position
                    .closed_lots
                    .iter()
                    .find(|x| x.lot_id.eq(&closed_lot_id)),
                None
            ));
        }

        order_connector.write().notify_logout(
            "Session-01".into(),
            "Session disconnected".to_owned(),
            logout_timestamp,
        );
        run_mock_deferred(&deferred);
    }
}
