use std::sync::Arc;

use index_maker::core::bits::SingleOrder;

pub enum Command {
    NewOrder(Arc<SingleOrder>),
}

pub struct SessionCommand {
    pub api_key: String,
    pub command: Command,
}