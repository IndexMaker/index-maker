use std::{collections::VecDeque, sync::Arc};

use index_maker::server::server::{Server, ServerEvent, ServerResponse};
use symm_core::core::functional::{IntoObservableManyVTable, NotificationHandler};

pub struct SolverOutputServer {
    pub responses: VecDeque<ServerResponse>,
}

impl SolverOutputServer {
    pub fn new() -> Self {
        Self {
            responses: VecDeque::new(),
        }
    }
}

impl Server for SolverOutputServer {
    fn respond_with(&mut self, response: ServerResponse) {
        self.responses.push_back(response);
    }

    fn initialize_shutdown(&mut self) {
        unimplemented!()
    }
}

impl IntoObservableManyVTable<Arc<ServerEvent>> for SolverOutputServer {
    fn add_observer(
        &mut self,
        _observer: Box<dyn NotificationHandler<Arc<ServerEvent>>>,
    ) {
        unimplemented!()
    }
}
