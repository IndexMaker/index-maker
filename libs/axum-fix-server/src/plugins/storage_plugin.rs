use eyre::{eyre, Result};
use parking_lot::RwLock;
use std::collections::HashMap;
use std::fs::{create_dir_all, OpenOptions};
use std::io::{self, Write};
use std::path::Path;
use symm_core::core::bits::Address;

pub struct StoragePlugin {
    sent_messages: RwLock<HashMap<(u32, Address), HashMap<u32, String>>>,
    received_messages: RwLock<HashMap<(u32, Address), HashMap<u32, String>>>,
}

impl StoragePlugin {
    pub fn new() -> Self {
        Self::verify_paths().expect("Failed to create storage directories");
        Self {
            sent_messages: RwLock::new(HashMap::new()),
            received_messages: RwLock::new(HashMap::new()),
        }
    }

    fn verify_paths() -> Result<()> {
        create_dir_all("data/Fix messages/Sent")?;
        create_dir_all("data/Fix messages/Received")?;
        Ok(())
    }

    pub fn store_sent_msg(
        &self,
        user_id: (u32, Address),
        seqnum: u32,
        msg: String,
    ) {
        {
            let mut sent = self.sent_messages.write();
            let user_map = sent.entry(user_id).or_insert_with(HashMap::new);
            user_map.insert(seqnum, msg.clone());
        }
        let _ = self.append_to_file("Sent", user_id, seqnum, &msg);
    }

    pub fn store_received_msg(
        &self,
        user_id: (u32, Address),
        seqnum: u32,
        msg: String,
    ) {
        {
            let mut received = self.received_messages.write();
            let user_map = received.entry(user_id).or_insert_with(HashMap::new);
            user_map.insert(seqnum, msg.clone());
        }
        let _ = self.append_to_file("Received", user_id, seqnum, &msg);
    }

    fn append_to_file(
        &self,
        kind: &str,
        user_id: (u32, Address),
        seqnum: u32,
        msg: &str,
    ) -> Result<()> {
        let base = Path::new("data").join("Fix messages").join(kind);
        let filename = format!("{}_{:x}.dat", user_id.0, user_id.1);
        let path = base.join(filename);

        if let Some(parent) = path.parent() {
            create_dir_all(parent)?;
        }

        let mut file = OpenOptions::new()
            .append(true)
            .create(true)
            .open(&path)
            .map_err(|e| eyre!("Failed to open file {}: {}", path.display(), e))?;

        let cleaned_msg = msg
            .replace(|c: char| c.is_whitespace() || c == '\t' || c == '\n' || c == '\r', "");

        writeln!(file, "{}: {}", seqnum, cleaned_msg)
            .map_err(|e| eyre!("Failed to write to file {}: {}", path.display(), e))?;

        Ok(())
    }
}