use std::{collections::VecDeque, sync::Arc};

use chrono::TimeDelta;
use parking_lot::RwLock;

use serde::{Deserialize, Serialize};
use symm_core::core::bits::{Address, ClientOrderId};

use super::solver_order::{SolverClientOrders, SolverOrder};

#[derive(Serialize, Deserialize)]
pub struct StoredSolverClientOrders {
    /// A map of all index orders from all clients
    client_orders: Vec<((u32, Address, ClientOrderId), Box<SolverOrder>)>,
    /// A map of queues with index order client IDs, so that we process them in that order
    client_order_queues: Vec<((u32, Address), VecDeque<ClientOrderId>)>,
    /// An internal notification queue, that we check on solver tick
    client_notify_queue: VecDeque<(u32, Address)>,
    /// A delay before we start processing client order
    client_wait_period: TimeDelta,
}

impl StoredSolverClientOrders {
    pub fn len(&self) -> usize {
        self.client_orders.len()
    }
}

impl From<SolverClientOrders> for StoredSolverClientOrders {
    fn from(value: SolverClientOrders) -> Self {
        Self {
            client_orders: value
                .client_orders
                .into_iter()
                .map(|(k, v)| (k, Box::new((&*v.read()).clone())))
                .collect(),
            client_order_queues: value.client_order_queues.into_iter().collect(),
            client_notify_queue: value.client_notify_queue.into_iter().collect(),
            client_wait_period: value.client_wait_period,
        }
    }
}

impl From<StoredSolverClientOrders> for SolverClientOrders {
    fn from(value: StoredSolverClientOrders) -> Self {
        Self {
            client_orders: value
                .client_orders
                .into_iter()
                .map(|(k, v)| (k, Arc::new(RwLock::new(*v))))
                .collect(),
            client_order_queues: value.client_order_queues.into_iter().collect(),
            client_notify_queue: value.client_notify_queue.into_iter().collect(),
            client_wait_period: value.client_wait_period,
        }
    }
}
