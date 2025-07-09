use std::{collections::HashSet, usize};

use eyre::{eyre, OptionExt, Result};
use itertools::Itertools;
use symm_core::core::bits::Symbol;
use tokio::sync::mpsc::UnboundedSender;

pub struct Subscriptions {
    subscription_sender: UnboundedSender<Symbol>,
    subscriptions: HashSet<Symbol>,
    subscriptions_taken: HashSet<Symbol>,
}

impl Subscriptions {
    pub fn new(subscription_sender: UnboundedSender<Symbol>) -> Self {
        Self {
            subscription_sender,
            subscriptions: HashSet::new(),
            subscriptions_taken: HashSet::new(),
        }
    }

    pub fn get_subscriptions(&self) -> &HashSet<Symbol> {
        &self.subscriptions
    }

    pub fn get_subscription_count(&self) -> usize {
        self.subscriptions.len()
    }

    pub fn get_subscriptions_taken(&self) -> usize {
        self.subscriptions_taken.len()
    }

    pub fn add_subscription_taken(&mut self, symbol: Symbol) -> Result<()> {
        self.subscriptions
            .contains(&symbol)
            .then_some(())
            .ok_or_eyre("Subscription not found")?;
        self.subscriptions_taken
            .insert(symbol)
            .then_some(())
            .ok_or_eyre("Subscription already taken")?;
        Ok(())
    }

    pub fn subscribe(&mut self, symbols: &[Symbol]) -> Result<()> {
        let (successes, failures): (Vec<_>, Vec<_>) = symbols
            .iter()
            .map(|symbol| {
                self.subscription_sender
                    .send(symbol.clone())
                    .map(|_| symbol.clone())
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
