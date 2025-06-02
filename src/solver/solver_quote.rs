use std::{
    collections::{hash_map::Entry, HashMap, VecDeque},
    sync::Arc,
};

use chrono::{DateTime, TimeDelta, Utc};
use parking_lot::RwLock;
use eyre::{eyre, OptionExt, Result};

use crate::core::bits::{Address, Amount, ClientQuoteId, Side, Symbol};

#[derive(Clone, Copy, Debug)]
pub enum SolverQuoteStatus {
    Open,
    Ready,
    MissingPrices,
    InvalidSymbol,
    MathOverflow,
    InvalidOrder,
}

pub struct SolverQuote {
    /// Chain ID
    pub chain_id: u32,

    /// On-chain wallet address
    pub address: Address,

    /// Cliend order ID
    pub client_quote_id: ClientQuoteId,

    /// An index symbol
    pub symbol: Symbol,

    /// Buy or Sell
    pub side: Side,

    /// Collateral amount to quote for
    pub collateral_amount: Amount,

    /// Quantity of an index possible to obtain
    pub quantity_possible: Amount,

    /// Time when last updated
    pub timestamp: DateTime<Utc>,

    /// Solver status
    pub status: SolverQuoteStatus,
}

pub struct SolverClientQuotes {
    client_quotes: HashMap<(u32, Address, ClientQuoteId), Arc<RwLock<SolverQuote>>>,
    client_quotes_queues: HashMap<(u32, Address), VecDeque<ClientQuoteId>>,
    client_notify_queue: VecDeque<(u32, Address)>,
    client_wait_period: TimeDelta,
}

impl SolverClientQuotes {
    pub fn new(client_wait_period: TimeDelta) -> Self {
        Self {
            client_quotes: HashMap::new(),
            client_quotes_queues: HashMap::new(),
            client_notify_queue: VecDeque::new(),
            client_wait_period,
        }
    }

    pub fn get_client_quote(
        &self,
        chain_id: u32,
        address: Address,
        client_quote_id: ClientQuoteId,
    ) -> Option<Arc<RwLock<SolverQuote>>> {
        self.client_quotes
            .get(&(chain_id, address, client_quote_id))
            .cloned()
    }

    pub fn get_next_client_quote(
        &mut self,
        timestamp: DateTime<Utc>,
    ) -> Option<Arc<RwLock<SolverQuote>>> {
        let ready_timestamp = timestamp - self.client_wait_period;
        let check_not_ready = |x: &SolverQuote| ready_timestamp < x.timestamp;
        if let Some(front) = self.client_notify_queue.front().cloned() {
            if let Some(queue) = self.client_quotes_queues.get_mut(&front) {
                if let Some(client_quote_id) = queue.front() {
                    if let Some(solver_quote) = self
                        .client_quotes
                        .get(&(front.0, front.1, client_quote_id.clone()))
                        .cloned()
                    {
                        let not_ready = check_not_ready(&solver_quote.read());
                        if !not_ready {
                            self.client_notify_queue.pop_front();
                            return Some(solver_quote);
                        }
                    }
                }
            }
        }
        None
    }

    pub fn add_client_quote(
        &mut self,
        chain_id: u32,
        address: Address,
        client_quote_id: ClientQuoteId,
        symbol: Symbol,
        side: Side,
        collateral_amount: Amount,
        timestamp: DateTime<Utc>,
    ) -> Result<()> {
        match self
            .client_quotes
            .entry((chain_id, address, client_quote_id.clone()))
        {
            Entry::Vacant(entry) => {
                let solver_order = Arc::new(RwLock::new(SolverQuote {
                    chain_id,
                    address,
                    client_quote_id: client_quote_id.clone(),
                    symbol,
                    side,
                    collateral_amount,
                    quantity_possible: Amount::ZERO,
                    timestamp,
                    status: SolverQuoteStatus::Open,
                }));
                entry.insert(solver_order.clone());

                Ok(())
            }
            Entry::Occupied(_) => Err(eyre!(
                "Duplicate Client Quote ID: [{}:{}] {}",
                chain_id,
                address,
                client_quote_id
            )),
        }?;

        let notify = match self.client_quotes_queues.entry((chain_id, address)) {
            Entry::Vacant(entry) => {
                entry.insert(VecDeque::from([client_quote_id.clone()]));
                true
            }
            Entry::Occupied(mut entry) => {
                entry.get_mut().push_back(client_quote_id.clone());
                false
            }
        };

        if notify {
            self.client_notify_queue.push_back((chain_id, address));
        }

        Ok(())
    }
    
    pub fn cancel_client_order(
        &mut self,
        chain_id: u32,
        address: Address,
        client_quote_id: ClientQuoteId,
    ) -> Result<()> {
        self.client_quotes
            .remove(&(chain_id, address, client_quote_id.clone()))
            .ok_or_eyre("Failed to remove entry")?;

        let notify = match self.client_quotes_queues.entry((chain_id, address)) {
            Entry::Occupied(mut entry) => {
                let mut notify = false;
                let queue = entry.get_mut();
                if let Some(front) = queue.front() {
                    if front.eq(&client_quote_id) {
                        queue.pop_front();
                        if queue.is_empty() {
                            entry.remove();
                        } else {
                            notify = true;
                        }
                    } else {
                        queue.retain(|x| !x.eq(&client_quote_id));
                    }
                } else {
                    eprintln!("(solver) Cancel order found empty index order queue for the user");
                    entry.remove();
                }
                notify
            }
            Entry::Vacant(_) => {
                eprintln!("(solver) Cancel order cannot find any index orders for the user");
                false
            }
        };
        if notify {
            self.client_notify_queue.push_back((chain_id, address));
        }
        Ok(())
    }
}

#[cfg(test)]
mod test {

    #[test]
    fn test_solver_quotes() {

    }
}
