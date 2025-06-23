use std::sync::Arc;

use symm_core::{core::bits::SingleOrder, order_sender::order_connector::SessionId};

pub enum Command {
    NewOrder(Arc<SingleOrder>),
}

pub struct SessionCommand {
    pub session_id: SessionId,
    pub command: Command,
}
