use std::sync::Arc;

use alloy_primitives::{bytes, Bytes};
use index_core::index::basket::Basket;

pub fn bytes_from_weights(basket: Arc<Basket>) -> Bytes {
    let _ = basket;
    bytes!("0x0000")
}

pub fn weights_from_bytes(data: Bytes) -> Basket {
    let _ = data;
    Basket {
        basket_assets: Vec::new(),
    }
}
