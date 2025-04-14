use std::sync::{
    atomic::{AtomicBool, Ordering}, mpsc::{channel, Receiver, Sender}, Arc
};

use alloy::primitives::address;

use crate::assets::asset::Asset;

use super::bits::{Address, Amount, Symbol};

pub fn get_mock_decimal(numeric_string: &str) -> Amount {
    numeric_string.try_into().unwrap()
}

pub fn get_mock_tolerance() -> Amount {
    get_mock_decimal("0.00001")
}

pub fn get_mock_asset_name_1() -> Symbol {
    "AX1".into()
}
pub fn get_mock_asset_name_2() -> Symbol {
    "AX2".into()
}
pub fn get_mock_asset_name_3() -> Symbol {
    "AX3".into()
}

pub fn get_mock_asset_1_arc() -> Arc<Asset> {
    Arc::new(Asset::new(get_mock_asset_name_1()))
}
pub fn get_mock_asset_2_arc() -> Arc<Asset> {
    Arc::new(Asset::new(get_mock_asset_name_2()))
}
pub fn get_mock_asset_3_arc() -> Arc<Asset> {
    Arc::new(Asset::new(get_mock_asset_name_3()))
}

pub fn get_mock_index_name_1() -> Symbol {
    "IX1".into()
}
pub fn get_mock_index_name_2() -> Symbol {
    "IX2".into()
}
pub fn get_mock_index_name_3() -> Symbol {
    "IX3".into()
}

pub fn get_mock_address_1() -> Address {
    address!("0xd8da6bf26964af9d7eed9e03e53415d37aa96045")
}

pub fn get_mock_channel<T>() -> (Sender<T>, Receiver<T>) {
    channel::<T>()
}

pub fn get_mock_setup_arc<T>(arc: &mut Arc<T>) -> &mut T {
    Arc::get_mut(arc).unwrap()
}

pub fn get_mock_atomic_bool_pair() -> (Arc<AtomicBool>, Arc<AtomicBool>) {
    let called_1 = Arc::new(AtomicBool::new(false));
    let called_2 = called_1.clone();
    (called_1, called_2)
}

pub fn flag_mock_atomic_bool(value: &AtomicBool) {
    value.store(true, Ordering::Relaxed);
}

pub fn reset_flag_mock_atomic_bool(value: &AtomicBool) {
    value.store(false, Ordering::Relaxed);
}

pub fn test_mock_atomic_bool(value: &AtomicBool) -> bool {
    value.load(Ordering::Relaxed)
}