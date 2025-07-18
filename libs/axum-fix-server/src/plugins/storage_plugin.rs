use eyre::Result;
use parking_lot::RwLock;
use std::collections::HashMap;
use std::fs::{create_dir_all, OpenOptions};
use std::io::Write;
use std::path::Path;
use symm_core::core::bits::Address;

pub struct StoragePlugin {
    sent_messages: RwLock<HashMap<(u32, Address), HashMap<u32, String>>>,
    received_messages: RwLock<HashMap<(u32, Address), HashMap<u32, String>>>,
}
