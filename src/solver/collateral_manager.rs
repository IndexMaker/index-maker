use std::{
    collections::{HashMap, VecDeque},
    sync::Arc,
};

use chrono::{DateTime, TimeDelta, Utc};
use itertools::Itertools;
use parking_lot::{Mutex, RwLock};

use eyre::{OptionExt, Result};
use safe_math::safe;

use crate::core::{
    bits::{Address, Amount, PaymentId},
    decimal_ext::DecimalExt,
};

use super::solver::{SetSolverOrderStatus, SolverOrder};

struct CollateralTransaction {
    payment_id: PaymentId,
    amount: Amount,
    timestamp: DateTime<Utc>,
}

pub struct CollateralPosition {
    /// On-chain address of the User
    address: Address,

    /// Total balance of credits less debits in force
    pub balance: Amount,

    /// Total amount of pending credits
    pending_cr: Amount,

    /// Total amount of pending debits
    pending_dr: Amount,

    /// Pending credits
    transactions_cr: VecDeque<CollateralTransaction>,

    /// Pending debits
    transactions_dr: VecDeque<CollateralTransaction>,

    /// Completed credits and debits
    completed_transactions: VecDeque<CollateralTransaction>,

    /// Last time we updated this position
    last_update_timestamp: DateTime<Utc>,
}

impl CollateralPosition {
    fn new(address: Address) -> Self {
        Self {
            address,
            balance: Amount::ZERO,
            pending_cr: Amount::ZERO,
            pending_dr: Amount::ZERO,
            transactions_cr: VecDeque::new(),
            transactions_dr: VecDeque::new(),
            completed_transactions: VecDeque::new(),
            last_update_timestamp: Default::default(),
        }
    }
}

pub trait CollateralManagerHost: SetSolverOrderStatus {}

pub struct CollateralManager {
    client_funds: RwLock<HashMap<Address, Arc<RwLock<CollateralPosition>>>>,
    ready_funds: Mutex<VecDeque<Arc<RwLock<CollateralPosition>>>>,
    index_orders: Mutex<VecDeque<Arc<RwLock<SolverOrder>>>>,
    fund_wait_period: TimeDelta,
}

impl CollateralManager {
    pub fn new(fund_wait_period: TimeDelta) -> Self {
        Self {
            fund_wait_period,
            client_funds: RwLock::new(HashMap::new()),
            ready_funds: Mutex::new(VecDeque::new()),
            index_orders: Mutex::new(VecDeque::new()),
        }
    }

    pub fn process_credits(
        &self,
        host: &dyn CollateralManagerHost,
        timestamp: DateTime<Utc>,
    ) -> Result<()> {
        let ready_timestamp = timestamp - self.fund_wait_period;
        let check_not_ready = |x: &CollateralTransaction| ready_timestamp < x.timestamp;
        let ready_funds = (|| {
            let mut funds = self.ready_funds.lock();
            let res = funds.iter().find_position(|p| {
                p.read()
                    .transactions_cr
                    .front()
                    .map(check_not_ready)
                    .unwrap_or(true)
            });
            if let Some((pos, _)) = res {
                funds.drain(..pos)
            } else {
                funds.drain(..)
            }
            .collect_vec()
        })();

        for fund in ready_funds {
            let mut fund_write = fund.write();
            let res = fund_write
                .transactions_cr
                .iter()
                .find_position(|x| check_not_ready(x));
            let completed = if let Some((pos, _)) = res {
                fund_write.transactions_cr.drain(..pos)
            } else {
                fund_write.transactions_cr.drain(..)
            }
            .collect_vec();
            fund_write.balance = completed
                .iter()
                .try_fold(fund_write.balance, |balance, tx| safe!(balance + tx.amount))
                .ok_or_eyre("Math Problem")?;
            fund_write.last_update_timestamp = ready_timestamp;
            fund_write.completed_transactions.extend(completed);
            println!(
                "New balance for {} {:0.5}",
                fund_write.address, fund_write.balance
            );
        }
        todo!("Find index orders that are now ready");

        Ok(())
    }

    pub fn handle_new_index_order(&self, solver_order: Arc<RwLock<SolverOrder>>) -> Result<()> {
        self.index_orders.lock().push_back(solver_order);
        Ok(())
    }

    pub fn handle_deposit(
        &self,
        address: Address,
        payment_id: PaymentId,
        amount: Amount,
        timestamp: DateTime<Utc>,
    ) -> Result<()> {
        let collateral_position = self
            .client_funds
            .write()
            .entry(address)
            .or_insert_with(|| Arc::new(RwLock::new(CollateralPosition::new(address))))
            .clone();

        (|| -> Result<()> {
            let mut p = collateral_position.write();

            p.transactions_cr.push_back(CollateralTransaction {
                payment_id,
                amount,
                timestamp,
            });
            p.pending_cr = safe!(p.pending_cr + amount).ok_or_eyre("Math Problem")?;
            p.last_update_timestamp = timestamp;
            Ok(())
        })()?;

        self.ready_funds.lock().push_back(collateral_position);
        Ok(())
    }

    pub fn handle_withdrawal(
        &self,
        address: Address,
        payment_id: PaymentId,
        amount: Amount,
        timestamp: DateTime<Utc>,
    ) -> Result<()> {
        let collateral_position = self
            .client_funds
            .write()
            .entry(address)
            .or_insert_with(|| Arc::new(RwLock::new(CollateralPosition::new(address))))
            .clone();

        (|| -> Result<()> {
            let mut p = collateral_position.write();

            p.transactions_dr.push_back(CollateralTransaction {
                payment_id,
                amount,
                timestamp,
            });
            p.pending_dr = safe!(p.pending_dr + amount).ok_or_eyre("Math Problem")?;
            p.last_update_timestamp = timestamp;
            Ok(())
        })()?;

        self.ready_funds.lock().push_back(collateral_position);
        Ok(())
    }

    pub fn get_funds(&self, address: &Address) -> Option<Arc<RwLock<CollateralPosition>>> {
        let client_funds = self.client_funds.read();
        client_funds.get(address).cloned()
    }
}
