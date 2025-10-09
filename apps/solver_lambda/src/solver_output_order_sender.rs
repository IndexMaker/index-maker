use std::{
    collections::{HashMap, VecDeque},
    sync::Arc,
};

use symm_core::{
    core::{
        bits::{Amount, SingleOrder, Symbol},
        functional::{
            IntoObservableSingleVTable, NotificationHandlerOnce, OneShotSingleObserver,
        },
    },
    order_sender::order_connector::{OrderConnector, OrderConnectorNotification, SessionId},
};

pub struct SolverOutputOrderSender {
    pub orders: VecDeque<(SessionId, Arc<SingleOrder>)>,
}

impl SolverOutputOrderSender {
    pub fn new() -> Self {
        Self {
            orders: VecDeque::new(),
        }
    }
}

impl OrderConnector for SolverOutputOrderSender {
    fn send_order(&mut self, session_id: SessionId, order: &Arc<SingleOrder>) -> eyre::Result<()> {
        let entry = (session_id, order.clone());
        self.orders.push_back(entry);
        Ok(())
    }

    fn get_balances(
        &self,
        _session_id: SessionId,
        _observer: OneShotSingleObserver<HashMap<Symbol, Amount>>,
    ) -> eyre::Result<()> {
        unimplemented!()
    }
}

impl IntoObservableSingleVTable<OrderConnectorNotification> for SolverOutputOrderSender {
    fn set_observer(
        &mut self,
        _observer: Box<dyn NotificationHandlerOnce<OrderConnectorNotification>>,
    ) {
        unimplemented!()
    }
}
