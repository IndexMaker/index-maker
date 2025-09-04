use std::{
    collections::{HashMap, VecDeque},
    sync::Arc,
};

use chrono::TimeDelta;
use parking_lot::RwLock;

use serde::{Deserialize, Serialize};
use symm_core::core::bits::{Address, ClientOrderId};

use super::solver_order::{SolverClientOrders, SolverOrder};

#[derive(Serialize, Deserialize)]
pub struct SolverClientOrdersSerde {
    /// A map of all index orders from all clients
    client_orders: HashMap<(u32, Address, ClientOrderId), Box<SolverOrder>>,
    /// A map of queues with index order client IDs, so that we process them in that order
    client_order_queues: HashMap<(u32, Address), VecDeque<ClientOrderId>>,
    /// An internal notification queue, that we check on solver tick
    client_notify_queue: VecDeque<(u32, Address)>,
    /// A delay before we start processing client order
    client_wait_period: TimeDelta,
}

impl SolverClientOrdersSerde {
    pub fn len(&self) -> usize {
        self.client_orders.len()
    }
}

impl From<SolverClientOrders> for SolverClientOrdersSerde {
    fn from(value: SolverClientOrders) -> Self {
        Self {
            client_orders: value
                .client_orders
                .into_iter()
                .map(|(k, v)| (k, Box::new((&*v.read()).clone())))
                .collect(),
            client_order_queues: value.client_order_queues,
            client_notify_queue: value.client_notify_queue,
            client_wait_period: value.client_wait_period,
        }
    }
}

impl From<SolverClientOrdersSerde> for SolverClientOrders {
    fn from(value: SolverClientOrdersSerde) -> Self {
        Self {
            client_orders: value
                .client_orders
                .into_iter()
                .map(|(k, v)| (k, Arc::new(RwLock::new(*v))))
                .collect(),
            client_order_queues: value.client_order_queues,
            client_notify_queue: value.client_notify_queue,
            client_wait_period: value.client_wait_period,
        }
    }
}
