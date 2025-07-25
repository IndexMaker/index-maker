use std::collections::HashSet;

use eyre::{eyre, OptionExt, Result};
use itertools::Itertools;
use tokio::sync::mpsc::UnboundedSender;

use crate::credentials::Credentials;

pub struct SubAccounts {
    subaccount_sender: UnboundedSender<Credentials>,
    subaccounts: HashSet<String>,
    subaccounts_taken: HashSet<String>,
}

impl SubAccounts {
    pub fn new(subaccount_sender: UnboundedSender<Credentials>) -> Self {
        Self {
            subaccount_sender,
            subaccounts: HashSet::new(),
            subaccounts_taken: HashSet::new(),
        }
    }

    pub fn get_subaccounts(&self) -> &HashSet<String> {
        &self.subaccounts
    }

    pub fn get_subaccount_count(&self) -> usize {
        self.subaccounts.len()
    }

    pub fn get_subaccounts_taken(&self) -> usize {
        self.subaccounts_taken.len()
    }

    pub fn add_subaccount_taken(&mut self, account_name: String) -> Result<()> {
        self.subaccounts
            .contains(&account_name)
            .then_some(())
            .ok_or_eyre("subaccount not found")?;
        self.subaccounts_taken
            .insert(account_name)
            .then_some(())
            .ok_or_eyre("subaccount already taken")?;
        Ok(())
    }

    pub fn logon(
        &mut self,
        multiple_credentials: impl IntoIterator<Item = Credentials>,
    ) -> Result<()> {
        let (successes, failures): (Vec<_>, Vec<_>) = multiple_credentials
            .into_iter()
            .map(|credentials| {
                let account_name = credentials.get_account_name();
                self.subaccount_sender
                    .send(credentials)
                    .map(|_| account_name)
            })
            .partition_result();

        if !failures.is_empty() {
            Err(eyre!(
                "subaccounts failed {}",
                failures.iter().map(|e| format!("{:?}", e)).join(";"),
            ))?;
        }

        self.subaccounts.extend(successes);
        Ok(())
    }
}
