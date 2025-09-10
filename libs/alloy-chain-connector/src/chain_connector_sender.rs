use std::{collections::VecDeque, sync::Arc};

use eyre::{eyre, OptionExt};
use parking_lot::{Mutex, RwLock as AtomicLock};

use crate::{command::Command, sessions::Sessions};

pub struct RealChainConnectorSender {
    sessions: Arc<AtomicLock<Sessions>>,
    account_names: Mutex<VecDeque<String>>,
}

impl RealChainConnectorSender {
    pub fn new(
        sessions: Arc<AtomicLock<Sessions>>,
        account_names: impl IntoIterator<Item = String>,
    ) -> Self {
        Self {
            sessions,
            account_names: Mutex::new(VecDeque::from_iter(account_names)),
        }
    }

    pub fn next_account_name(&self) -> Option<String> {
        let res = self.account_names.lock().front().cloned();
        self.account_names.lock().rotate_left(1);
        res
    }

    pub fn send_command(&self, command: Command) -> eyre::Result<()> {
        if let Some(account_name) = self.next_account_name() {
            self.send_command_to_session(&account_name, command)?;
        } else {
            Err(eyre!("Failed to find account for send collateral"))?;
        }
        Ok(())
    }

    fn send_command_to_session(&self, account_name: &str, command: Command) -> eyre::Result<()> {
        let sessions_read = self.sessions.read();
        let issuer = sessions_read
            .get_session(account_name)
            .ok_or_eyre("Session not found")?;

        issuer.send_command(command)
    }
}
