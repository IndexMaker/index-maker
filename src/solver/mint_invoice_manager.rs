use std::{
    collections::{HashMap, VecDeque},
    ops::Deref,
    sync::Arc,
};

use chrono::{DateTime, Utc};
use itertools::Itertools;
use serde::{Deserialize, Serialize};
use serde_json::json;
use symm_core::core::{
    bits::{Address, ClientOrderId, Symbol},
    persistence::{Persist, Persistence},
};

use crate::solver::mint_invoice::MintInvoice;

#[derive(Serialize, Deserialize)]
pub struct GetInvoiceData {
    pub chain_id: u32,
    pub address: Address,
    pub invoice: MintInvoice,
}

#[derive(Serialize, Deserialize)]
pub struct GetInvoicesData {
    pub chain_id: u32,
    pub address: Address,
    pub client_order_id: ClientOrderId,
    pub symbol: Symbol,
    pub timestamp: DateTime<Utc>,
}

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
    ) -> eyre::Result<Option<GetInvoiceData>> {
        let Some(invoices) = self.user_invoices.get(&(chain_id, address)) else {
            return Ok(None);
        };

        let Some(invoice) = invoices
            .iter()
            .find(|x| x.client_order_id.eq(client_order_id))
        else {
            return Ok(None);
        };

        Ok(Some(GetInvoiceData {
            chain_id,
            address,
            invoice: invoice.deref().clone(),
        }))
    }

    pub fn get_invoices_in_date_range(
        &self,
        from_date: DateTime<Utc>,
        to_date: DateTime<Utc>,
    ) -> eyre::Result<Vec<GetInvoicesData>> {
        let invoices = self
            .all_invoices
            .iter()
            .rev()
            .take_while_inclusive(|(.., x)| from_date <= x.timestamp)
            .map(|(chain_id, address, invoice)| (chain_id, address, invoice))
            .collect_vec();

        let invoices = invoices
            .into_iter()
            .rev()
            .take_while_inclusive(|(.., x)| x.timestamp <= to_date)
            .map(|(chain_id, address, invoice)| GetInvoicesData {
                chain_id: *chain_id,
                address: *address,
                client_order_id: invoice.client_order_id.clone(),
                symbol: invoice.symbol.clone(),
                timestamp: invoice.timestamp,
            })
            .collect_vec();

        Ok(invoices)
    }
}

impl Persist for MintInvoiceManager {
    fn load(&mut self) -> eyre::Result<()> {
        self.user_invoices = HashMap::new();
        self.all_invoices = VecDeque::new();

        if let Some(mut value) = self.persistence.load_value()? {
            if let Some(invoices) = value.get_mut("invoices") {
                let all_invoices: Vec<(u32, Address, MintInvoice)> =
                    serde_json::from_value(invoices.take())?;

                for (chain_id, address, invoice) in all_invoices {
                    self.add_invoice(chain_id, address, invoice)?
                }
            }
        }
        Ok(())
    }

    fn store(&self) -> eyre::Result<()> {
        self.persistence
            .store_value(json!({"invoices": self.all_invoices}))?;
        Ok(())
    }
}
