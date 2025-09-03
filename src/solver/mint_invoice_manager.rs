use std::{
    collections::{HashMap, VecDeque},
    ops::Deref,
    sync::Arc,
};

use chrono::{Duration, Utc};
use itertools::Itertools;
use symm_core::core::{
    bits::{Address, ClientOrderId},
    persistence::{Persist, Persistence},
};

use crate::solver::mint_invoice::MintInvoice;

pub struct MintInvoiceManager {
    persistence: Arc<dyn Persistence + Send + Sync + 'static>,
    user_invoices: HashMap<(u32, Address), VecDeque<Arc<MintInvoice>>>,
    all_invoices: VecDeque<(u32, Address, Arc<MintInvoice>)>,
}

impl MintInvoiceManager {
    pub fn new(persistence: Arc<dyn Persistence + Send + Sync + 'static>) -> Self {
        Self {
            persistence,
            user_invoices: HashMap::new(),
            all_invoices: VecDeque::new(),
        }
    }

    pub fn add_invoice(
        &mut self,
        chain_id: u32,
        address: Address,
        invoice: MintInvoice,
    ) -> eyre::Result<()> {
        let invoices = self.user_invoices.entry((chain_id, address)).or_default();
        let invoice = Arc::new(invoice);
        invoices.push_back(invoice.clone());
        self.all_invoices.push_back((chain_id, address, invoice));
        Ok(())
    }

    pub fn get_invoice(
        &self,
        chain_id: u32,
        address: Address,
        client_order_id: &ClientOrderId,
    ) -> eyre::Result<Option<MintInvoice>> {
        let Some(invoices) = self.user_invoices.get(&(chain_id, address)) else {
            return Ok(None);
        };

        let Some(invoice) = invoices
            .iter()
            .find(|x| x.client_order_id.eq(client_order_id))
        else {
            return Ok(None);
        };

        Ok(Some(invoice.deref().clone()))
    }

    pub fn get_recent_invoices(
        &self,
        period: Duration,
    ) -> eyre::Result<Vec<(u32, Address, ClientOrderId)>> {
        let from_when = Utc::now() - period;

        let invoices = self
            .all_invoices
            .iter()
            .rev()
            .take_while(|(.., x)| from_when < x.timestamp)
            .map(|(chain_id, address, invoice)| {
                (*chain_id, *address, invoice.client_order_id.clone())
            })
            .collect_vec();

        Ok(invoices)
    }
}

impl Persist for MintInvoiceManager {
    fn load(&mut self) -> eyre::Result<()> {
        todo!()
    }

    fn store(&self) -> eyre::Result<()> {
        todo!()
    }
}
