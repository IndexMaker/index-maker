use std::{collections::HashSet, usize};

use eyre::{eyre, Result};
use itertools::Itertools;
use symm_core::market_data::market_data_connector::Subscription;
use tokio::sync::mpsc::UnboundedSender;

pub struct Subscriptions {
    subscription_sender: UnboundedSender<Subscription>,
    subscriptions: HashSet<Subscription>,
}

impl Subscriptions {
    pub fn new(subscription_sender: UnboundedSender<Subscription>) -> Self {
        Self {
            subscription_sender,
            subscriptions: HashSet::new(),
        }
    }

    pub fn get_subscriptions(&self) -> &HashSet<Subscription> {
        &self.subscriptions
    }

    pub fn get_subscription_count(&self) -> usize {
        self.subscriptions.len()
    }

    pub fn subscribe(&mut self, subscriptions: &[Subscription]) -> Result<()> {
        let (successes, failures): (Vec<_>, Vec<_>) = subscriptions
            .iter()
            .map(|subscription| {
                self.subscription_sender
                    .send(subscription.clone())
                    .map(|_| subscription.clone())
            })
            .partition_result();

        if !failures.is_empty() {
            Err(eyre!(
                "Subscriptions failed {}",
                failures.iter().map(|e| format!("{:?}", e)).join(";"),
            ))?;
        }

        self.subscriptions.extend(successes.into_iter());
        Ok(())
    }
}
