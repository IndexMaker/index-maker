use std::{
    collections::{hash_map::Entry, HashMap, VecDeque},
    sync::Arc,
};

use chrono::{DateTime, TimeDelta, Utc};
use eyre::{eyre, OptionExt, Result};
use itertools::Itertools;
use parking_lot::RwLock;
use safe_math::safe;

use serde::{Deserialize, Serialize};
use serde_json::json;
use symm_core::{
    core::{
        bits::{Address, Amount, ClientOrderId, PaymentId, Side, Symbol},
        decimal_ext::DecimalExt,
        telemetry::{TracingData, WithTracingData},
    },
    order_sender::position::LotId,
};

#[derive(Clone, Copy, Debug)]
pub enum SolverOrderStatus {
    Open,
    ManageCollateral,
    Ready,
    Engaged,
    PartlyMintable,
    FullyMintable,
    Minted,
    InvalidSymbol,
    InvalidOrder,
    InvalidCollateral,
    ServiceUnavailable,
    InternalError,
}

/// Solver's view of the Index Order
pub struct SolverOrder {
    // Chain ID
    pub chain_id: u32,

    /// On-chain address of the User
    pub address: Address,

    // ID of the NewOrder request
    pub client_order_id: ClientOrderId,

    /// An ID of the on-chain payment
    pub payment_id: Option<PaymentId>,

    /// Symbol of an Index
    pub symbol: Symbol,

    /// Side of an order
    pub side: Side,

    /// Collateral remaining on the order to complete
    pub remaining_collateral: Amount,

    /// Collateral solver has engaged so far
    pub engaged_collateral: Amount,

    /// Collateral carried over from last batch
    pub collateral_carried: Amount,

    /// Collateral spent by solver
    pub collateral_spent: Amount,

    /// Quantity filled by subsequent batch order fills
    pub filled_quantity: Amount,

    /// Time when last updated
    pub timestamp: DateTime<Utc>,

    /// Solver status
    pub status: SolverOrderStatus,

    /// All asset lots allocated to this Index Order
    pub lots: Vec<SolverOrderAssetLot>,

    /// Telemetry data
    pub tracing_data: TracingData,
}

impl SolverOrder {
    pub fn get_key(&self) -> (u32, Address, ClientOrderId) {
        (self.chain_id, self.address, self.client_order_id.clone())
    }
}

impl WithTracingData for SolverOrder {
    fn get_tracing_data_mut(&mut self) -> &mut TracingData {
        &mut self.tracing_data
    }

    fn get_tracing_data(&self) -> &TracingData {
        &self.tracing_data
    }
}

/// When we fill Index Orders from executed batches, we need to allocate some
/// portion of what was executed to each index order in the batch using
/// contribution factor.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SolverOrderAssetLot {
    /// Associated Inventory lot ID
    pub lot_id: LotId,

    /// Symbol of an asset
    pub symbol: Symbol,

    /// Executed price
    pub price: Amount,

    /// Original quantity
    pub original_quantity: Amount,

    // Remaining quantity in the lot not assigned to any index order
    pub remaining_quantity: Amount,

    /// Original execution fee
    pub original_fee: Amount,

    /// Quantity allocated to index order
    pub assigned_quantity: Amount,
    
    /// Execution fee allocated to index order
    pub assigned_fee: Amount,

    /// Time when lot was created
    pub created_timestamp: DateTime<Utc>,

    /// Time when lot was assigned to index order
    pub assigned_timestamp: DateTime<Utc>,
}

impl SolverOrderAssetLot {
    pub fn compute_collateral_spent(&self) -> Option<Amount> {
        safe!(safe!(self.assigned_quantity * self.price) + self.assigned_fee)
    }
}

pub struct SolverClientOrders {
    /// A map of all index orders from all clients
    client_orders: HashMap<(u32, Address, ClientOrderId), Arc<RwLock<SolverOrder>>>,
    /// A map of queues with index order client IDs, so that we process them in that order
    client_order_queues: HashMap<(u32, Address), VecDeque<ClientOrderId>>,
    /// An internal notification queue, that we check on solver tick
    client_notify_queue: VecDeque<(u32, Address)>,
    /// A delay before we start processing client order
    client_wait_period: TimeDelta,
}

impl SolverClientOrders {
    pub fn new(client_wait_period: TimeDelta) -> Self {
        Self {
            client_orders: HashMap::new(),
            client_order_queues: HashMap::new(),
            client_notify_queue: VecDeque::new(),
            client_wait_period,
        }
    }

    pub fn get_client_order(
        &self,
        chain_id: u32,
        address: Address,
        client_order_id: ClientOrderId,
    ) -> Option<Arc<RwLock<SolverOrder>>> {
        self.client_orders
            .get(&(chain_id, address, client_order_id))
            .cloned()
    }

    pub fn get_next_client_order(
        &mut self,
        timestamp: DateTime<Utc>,
    ) -> Option<Arc<RwLock<SolverOrder>>> {
        let ready_timestamp = timestamp - self.client_wait_period;
        let check_ready = move |x: &SolverOrder| x.timestamp <= ready_timestamp;

        if let Some(front) = self.client_notify_queue.front().cloned() {
            if let Some(queue) = self.client_order_queues.get_mut(&front) {
                if let Some(client_order_id) = queue.front() {
                    if let Some(solver_order) = self
                        .client_orders
                        .get(&(front.0, front.1, client_order_id.clone()))
                        .cloned()
                    {
                        // NOTE: The client_queue has clients ordered in FIFO so
                        // first in the queue will have lowest timestamp. If we
                        // process first in the queue, we can move to second etc.
                        // If we don't process first, then there is no point moving
                        // to second, as their timestamp will be higher.
                        if check_ready(&solver_order.read()) {
                            self.client_notify_queue.pop_front();
                            return Some(solver_order);
                        }
                    }
                }
            }
        }
        None
    }

    pub fn put_back(&mut self, solver_order: Arc<RwLock<SolverOrder>>, timestamp: DateTime<Utc>) {
        let mut order_upread = solver_order.upgradable_read();
        let chain_id = order_upread.chain_id;
        let address = order_upread.address;

        order_upread.with_upgraded(|order_write| {
            order_write.status = SolverOrderStatus::Open;
            order_write.timestamp = timestamp;
        });

        let client_order_id = &order_upread.client_order_id;

        tracing::warn!(
            %chain_id,
            %address,
            %client_order_id,
            "Order put back into the queue. Will retry in {}s", self.client_wait_period.as_seconds_f64());

        self.client_notify_queue.push_back((chain_id, address));
    }

    pub fn add_client_order(
        &mut self,
        chain_id: u32,
        address: Address,
        client_order_id: ClientOrderId,
        symbol: Symbol,
        side: Side,
        collateral_amount: Amount,
        timestamp: DateTime<Utc>,
    ) -> Result<()> {
        match self
            .client_orders
            .entry((chain_id, address, client_order_id.clone()))
        {
            Entry::Vacant(entry) => {
                let solver_order = Arc::new(RwLock::new(SolverOrder {
                    chain_id,
                    address,
                    client_order_id: client_order_id.clone(),
                    payment_id: None,
                    symbol: symbol.clone(),
                    side,
                    remaining_collateral: collateral_amount,
                    engaged_collateral: Amount::ZERO,
                    collateral_carried: Amount::ZERO,
                    collateral_spent: Amount::ZERO,
                    filled_quantity: Amount::ZERO,
                    timestamp,
                    status: SolverOrderStatus::Open,
                    lots: Vec::new(),
                    tracing_data: TracingData::from_current_context(),
                }));
                entry.insert(solver_order);

                Ok(())
            }
            Entry::Occupied(_) => Err(eyre!(
                "Duplicate Client Order ID: [{}:{}] {}",
                chain_id,
                address,
                client_order_id
            )),
        }?;

        // We want to ensure that only one index order from given user
        // is processed at one time.
        let notify = match self.client_order_queues.entry((chain_id, address)) {
            Entry::Vacant(entry) => {
                // Current order is kept in the queue for as long as it is
                // still processed, i.e. until it's minted or cancelled.
                entry.insert(VecDeque::from([client_order_id.clone()]));
                true
            }
            Entry::Occupied(mut entry) => {
                // User sent another index order, and we will store it
                // in the queue, and process once we finished with
                // current index order for that user.
                entry.get_mut().push_back(client_order_id.clone());
                false
            }
        };

        if notify {
            // We use client queue to notify ourselves so that solve()
            // will pick order from the queue above at next tick.
            self.client_notify_queue.push_back((chain_id, address));
        }
        tracing::info!(
            %chain_id,
            %address,
            %client_order_id,
            %symbol,
            client_order_queue = %json!(
                self.client_order_queues.get(&(chain_id, address)).iter().map(|x| x).collect_vec()
            ),
            client_notify_queue = %json!(
                self.client_notify_queue
            ),
            "Client Orders");

        Ok(())
    }

    pub fn cancel_client_order(
        &mut self,
        chain_id: u32,
        address: Address,
        client_order_id: ClientOrderId,
    ) -> Result<()> {
        self.client_orders
            .remove(&(chain_id, address, client_order_id.clone()))
            .ok_or_eyre("Failed to remove entry")?;

        // Note we can have two possible cases:
        // 1. Order in front of FIFO queue is cancelled
        // 2. Order in the middle of FIFO queue is cancelled
        // Case 1. means that order is currently processed, and it could have
        // been cancelled for two reasons:
        //  a) it was minted
        //  b) user cancelled it
        // Case 2. means user cancelled it
        let notify = match self.client_order_queues.entry((chain_id, address)) {
            Entry::Occupied(mut entry) => {
                let mut notify = false;
                let queue = entry.get_mut();
                if let Some(front) = queue.front() {
                    if front.eq(&client_order_id) {
                        queue.pop_front();
                        if queue.is_empty() {
                            // This means that there is no more index orders
                            // for that user
                            entry.remove();
                        } else {
                            // Note we always keep current client_order_id in the
                            // queue, so that when we receive NewIndexOrder we know
                            // that current order is still being worked on.
                            notify = true;
                        }
                    } else {
                        queue.retain(|x| !x.eq(&client_order_id));
                    }
                } else {
                    // This is rather unexpected, as we would remove queue from map
                    // when it becomes empty.
                    tracing::warn!("Cancel order found empty index order queue for the user");
                    entry.remove();
                }
                notify
            }
            Entry::Vacant(_) => {
                // This is rather unexpected, as we would add any new index orders to
                // a queue and cancelling should always find a match in the queue.
                tracing::warn!("Cancel order cannot find any index orders for the user");
                false
            }
        };
        if notify {
            self.client_notify_queue.push_back((chain_id, address));
        }
        Ok(())
    }

    pub fn update_client_order(
        &mut self,
        chain_id: u32,
        address: Address,
        client_order_id: ClientOrderId,
        collateral_removed: Amount,
        timestamp: DateTime<Utc>,
    ) -> Result<()> {
        let mut cleanup = false;
        if let Some(order) = self
            .client_orders
            .get(&(chain_id, address, client_order_id))
        {
            let mut order_write = order.write();

            // Note that we update our solver view when we have a fill, so
            // the value of IndexOrderEvent::UpdateIndexOrder::collateral_remaining
            // wouldn't be adequate. It is true that Index Order Manager should
            // see same value, because after fill it should update it same way, but
            // we don't have any guarantees about when this event was published
            // and it could have been published before the fill, and that would
            // overwrite our local calculation. That is why we apply delta, which is
            // IndexOrderEvent::UpdateIndexOrder::collateral_removed
            order_write.remaining_collateral =
                safe!(order_write.remaining_collateral - collateral_removed)
                    .ok_or_eyre("Math Problem")?;

            order_write.timestamp = timestamp;

            if let SolverOrderStatus::Open = order_write.status {
                // Note that this only applies to orders with Open status. We wouldn't
                // have any entry in the client_queue for user for whom we are already
                // processing an order, and any next order will be added to queue for
                // that user in stored in client_order_queues. Also note that we added
                // check is status == Open purely for performance reasons, because if
                // code behaves as expected, the client_queue wouldn't have any entry
                // for this user.
                cleanup = true;
            }

            Ok(())
        } else {
            Err(eyre!("(solver) Handle order update: Missing order"))
        }?;

        if cleanup {
            // We want to ensure that any update to an order that can be picked up
            // by serve_more_clients() follows the timestamp rules, i.e. if user
            // cancelled quantity on the order, then we put that user at the end of
            // client_queue, because orders of other clients have lower timestamps
            // than this update.
            let key = (chain_id, address);
            if let Some((pos, _)) = self
                .client_notify_queue
                .iter()
                .find_position(|&x| x.eq(&key))
            {
                if self.client_notify_queue.len() > 1 {
                    self.client_notify_queue.remove(pos);
                    self.client_notify_queue.push_back(key);
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod test {
    use chrono::{TimeDelta, Utc};
    use rust_decimal::dec;

    use symm_core::core::{
        bits::Side,
        test_util::{get_mock_address_1, get_mock_asset_name_1},
    };

    use super::SolverClientOrders;

    /// Test SolverClientOrders - A Solver side IndexOrder manager
    /// --------
    /// Manage Solver Orders (Solver view of Index Orders w/ embedded Solver
    /// state) following these rules:
    /// - Orders from single user must be executed in the same sequence as they
    /// arrived, so that when order is put back, it has to be done as the next
    /// order for that user.
    /// - Only single order from single user can be actively executed, and all
    /// other orders from that user need to wait.
    /// - Orders from multiple users will be executed simultaneously.
    ///
    #[test]
    fn test_solver_orders() {
        let client_wait_period = TimeDelta::seconds(10);

        let mut timestamp = Utc::now();
        let mut solver_orders = SolverClientOrders::new(client_wait_period);

        // Register new client order for first user
        solver_orders
            .add_client_order(
                1,
                get_mock_address_1(),
                "C-01".into(),
                get_mock_asset_name_1(),
                Side::Buy,
                dec!(1000.0),
                timestamp,
            )
            .unwrap();

        // Register another client order for first user
        solver_orders
            .add_client_order(
                1,
                get_mock_address_1(),
                "C-02".into(),
                get_mock_asset_name_1(),
                Side::Buy,
                dec!(1000.0),
                timestamp,
            )
            .unwrap();

        // Register another client order from different user
        solver_orders
            .add_client_order(
                2,
                get_mock_address_1(),
                "C-01".into(),
                get_mock_asset_name_1(),
                Side::Buy,
                dec!(1000.0),
                timestamp,
            )
            .unwrap();

        // Should not give any order
        let order = solver_orders.get_next_client_order(timestamp);
        assert!(matches!(order, None));

        // Order should be in the map
        let order = solver_orders.get_client_order(1, get_mock_address_1(), "C-01".into());
        assert!(matches!(order, Some(..)));

        // Order should be in the map
        let order = solver_orders.get_client_order(1, get_mock_address_1(), "C-02".into());
        assert!(matches!(order, Some(..)));

        // Order should be in the map
        let order = solver_orders.get_client_order(2, get_mock_address_1(), "C-01".into());
        assert!(matches!(order, Some(..)));

        // Should give first order back
        timestamp += client_wait_period;
        let order = solver_orders.get_next_client_order(timestamp);
        assert!(matches!(order, Some(..)));
        if let Some(order) = order {
            assert_eq!(order.read().chain_id, 1);
            assert_eq!(order.read().client_order_id, "C-01".into());
        }

        // Order should be in the map
        let order = solver_orders.get_client_order(1, get_mock_address_1(), "C-01".into());
        assert!(matches!(order, Some(..)));

        // Put order back into the queue
        solver_orders.put_back(order.unwrap(), timestamp);
        timestamp += client_wait_period;

        // Should give an order of second user back
        let order = solver_orders.get_next_client_order(timestamp);
        assert!(matches!(order, Some(..)));
        if let Some(order) = order {
            assert_eq!(order.read().chain_id, 2);
            assert_eq!(order.read().client_order_id, "C-01".into());
        }

        // Should again give first order of first user back
        let order = solver_orders.get_next_client_order(timestamp);
        assert!(matches!(order, Some(..)));
        if let Some(order) = order {
            assert_eq!(order.read().chain_id, 1);
            assert_eq!(order.read().client_order_id, "C-01".into());
        }

        // Should not give any order
        let order = solver_orders.get_next_client_order(timestamp);
        assert!(matches!(order, None));

        // Should cancel first order of first user
        solver_orders
            .cancel_client_order(1, get_mock_address_1(), "C-01".into())
            .unwrap();

        // Should give second order of first user back
        let order = solver_orders.get_next_client_order(timestamp);
        assert!(matches!(order, Some(..)));
        if let Some(order) = order {
            assert_eq!(order.read().chain_id, 1);
            assert_eq!(order.read().client_order_id, "C-02".into());
        }

        // Should cancel order of second user
        solver_orders
            .cancel_client_order(2, get_mock_address_1(), "C-01".into())
            .unwrap();

        // Should not give any order
        let order = solver_orders.get_next_client_order(timestamp);
        assert!(matches!(order, None));
    }
}
