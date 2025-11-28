use std::sync::Arc;

use alloy_primitives::U256;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::json;
use symm_core::core::{
    bits::{Address, Amount, ClientOrderId, PaymentId, Symbol},
    persistence::{Persist, Persistence},
};

use crate::solver::mint_invoice::MintInvoice;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct InvoiceMetadata {
    pub chain_id: u32,
    pub address: Address,
    pub client_order_id: ClientOrderId,
    pub payment_id: PaymentId,
    pub seq_num: U256,
    pub symbol: Symbol,
    pub filled_quantity: Amount,
    pub total_amount: Amount,
    pub amount_paid: Amount,
    pub amount_remaining: Amount,
    pub management_fee: Amount,
    pub assets_value: Amount,
    pub exchange_fee: Amount,
    pub fill_rate: Amount,
    pub timestamp: DateTime<Utc>,
    // NOTE: No lots, no position - those are stored in individual files
}

impl From<&MintInvoice> for InvoiceMetadata {
    fn from(invoice: &MintInvoice) -> Self {
        Self {
            chain_id: 0, // Will be set by add_invoice
            address: Address::default(), // Will be set by add_invoice
            client_order_id: invoice.client_order_id.clone(),
            payment_id: invoice.payment_id.clone(),
            seq_num: invoice.seq_num,
            symbol: invoice.symbol.clone(),
            filled_quantity: invoice.filled_quantity.clone(),
            total_amount: invoice.total_amount,
            amount_paid: invoice.amount_paid,
            amount_remaining: invoice.amount_remaining,
            management_fee: invoice.management_fee,
            assets_value: invoice.assets_value,
            exchange_fee: invoice.exchange_fee,
            fill_rate: invoice.fill_rate,
            timestamp: invoice.timestamp,
        }
    }
}

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
    pub payment_id: PaymentId,
    pub seq_num: U256,
    pub symbol: Symbol,
    pub filled_quantity: Amount,
    pub total_amount: Amount,
    pub amount_paid: Amount,
    pub amount_remaining: Amount,
    pub management_fee: Amount,
    pub assets_value: Amount,
    pub exchange_fee: Amount,
    pub fill_rate: Amount,
    pub timestamp: DateTime<Utc>,
}

pub struct MintInvoiceManager {
    persistence: Arc<dyn Persistence + Send + Sync + 'static>,
    
    // Store ALL metadata in memory (loaded from root json)
    // This is lightweight - no lots, no positions
    metadata: Vec<InvoiceMetadata>,
}

impl MintInvoiceManager {
    pub fn new(persistence: Arc<dyn Persistence + Send + Sync + 'static>) -> Self {
        Self {
            persistence,
            metadata: Vec::new(),
        }
    }

    /// Create a unique key for an invoice
    fn create_invoice_key(
        chain_id: u32,
        address: &Address,
        client_order_id: &ClientOrderId,
    ) -> String {
        format!(
            "chain_{}/address_{}/order_{}",
            chain_id,
            address,
            client_order_id
        )
    }

    /// Add new invoice
    pub fn add_invoice(
        &mut self,
        chain_id: u32,
        address: Address,
        invoice: MintInvoice,
    ) -> eyre::Result<()> {
        // 1. Store full invoice (with lots & positions) in individual file
        let key = Self::create_invoice_key(chain_id, &address, &invoice.client_order_id);
        let child_persistence = self.persistence.child(key)?;
        
        let invoice_value = serde_json::to_value(&invoice)?;
        child_persistence.store_value(invoice_value)?;

        // 2. Add metadata (without lots & positions) to root
        let mut meta = InvoiceMetadata::from(&invoice);
        meta.chain_id = chain_id;
        meta.address = address;
        self.metadata.push(meta);

        // 3. Save root json (small - just metadata)
        self.store()?;

        Ok(())
    }

    /// Get specific invoice (loads full data including lots & positions)
    pub fn get_invoice(
        &self,
        chain_id: u32,
        address: Address,
        client_order_id: &ClientOrderId,
    ) -> eyre::Result<Option<GetInvoiceData>> {
        // Check if invoice exists in metadata
        let exists = self.metadata.iter().any(|m| 
            m.chain_id == chain_id 
            && m.address == address 
            && m.client_order_id == *client_order_id
        );

        if !exists {
            return Ok(None);
        }

        // Load full invoice from individual file
        let key = Self::create_invoice_key(chain_id, &address, client_order_id);
        let child_persistence = self.persistence.child(key)?;

        if let Some(invoice_value) = child_persistence.load_value()? {
            let invoice: MintInvoice = serde_json::from_value(invoice_value)?;
            
            Ok(Some(GetInvoiceData {
                chain_id,
                address,
                invoice,
            }))
        } else {
            Ok(None)
        }
    }

    /// Query invoices by date range (uses metadata only - no file loading!)
    pub fn get_invoices_in_date_range(
        &self,
        from_date: DateTime<Utc>,
        to_date: DateTime<Utc>,
    ) -> eyre::Result<Vec<GetInvoicesData>> {
        // Filter metadata only - no file I/O needed!
        let results = self
            .metadata
            .iter()
            .filter(|m| m.timestamp >= from_date && m.timestamp <= to_date)
            .map(|m| GetInvoicesData {
                chain_id: m.chain_id,
                address: m.address,
                client_order_id: m.client_order_id.clone(),
                payment_id: m.payment_id.clone(),
                seq_num: m.seq_num,
                symbol: m.symbol.clone(),
                filled_quantity: m.filled_quantity.clone(),
                total_amount: m.total_amount,
                amount_paid: m.amount_paid,
                amount_remaining: m.amount_remaining,
                management_fee: m.management_fee,
                assets_value: m.assets_value,
                exchange_fee: m.exchange_fee,
                fill_rate: m.fill_rate,
                timestamp: m.timestamp,
            })
            .collect();

        Ok(results)
    }
}

impl Persist for MintInvoiceManager {
    /// Load metadata from root json
    fn load(&mut self) -> eyre::Result<()> {
        self.metadata = Vec::new();
        if let Some(mut value) = self.persistence.load_value()? {
            if let Some(metadata) = value.get_mut("metadata") {
                self.metadata = serde_json::from_value(metadata.take())?;
            }
        }
        Ok(())
    }

    /// Store metadata to root json
    fn store(&self) -> eyre::Result<()> {
        // Store only metadata (no lots, no positions)
        self.persistence
            .store_value(json!({ "metadata": self.metadata }))?;
        
        Ok(())
    }
}


#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use symm_core::core::persistence::util::JsonFilePersistence;

    #[test]
    fn test_load_new_format() -> eyre::Result<()> {
        let file_path = PathBuf::from("persistence/InvoiceManager.json");
        
        if !file_path.exists() {
            println!("Skipping test - InvoiceManager.json not found");
            return Ok(());
        }

        let persistence = Arc::new(JsonFilePersistence::new(&file_path));
        let mut manager = MintInvoiceManager::new(persistence);

        manager.load()?;

        println!("Loaded {} invoices", manager.metadata.len());
        
        assert!(manager.metadata.len() >= 3, "Should have loaded at least 3 invoices");

        for meta in &manager.metadata {
            assert_eq!(meta.chain_id, 8453);
            println!("  - {}: {}", meta.client_order_id, meta.symbol);
        }

        Ok(())
    }

    #[test]
    fn test_get_invoice_with_full_data() -> eyre::Result<()> {
        let file_path = PathBuf::from("persistence/InvoiceManager.json");
        
        if !file_path.exists() {
            println!("Skipping test - InvoiceManager.json not found");
            return Ok(());
        }

        let persistence = Arc::new(JsonFilePersistence::new(&file_path));
        let mut manager = MintInvoiceManager::new(persistence);
        manager.load()?;

        if let Some(first_meta) = manager.metadata.first() {
            println!("Retrieving invoice: {}", first_meta.client_order_id);
            
            let result = manager.get_invoice(
                first_meta.chain_id,
                first_meta.address,
                &first_meta.client_order_id
            )?;
            
            assert!(result.is_some(), "Should retrieve invoice");
            
            let invoice_data = result.unwrap();
            assert_eq!(invoice_data.invoice.client_order_id, first_meta.client_order_id);
            assert_eq!(invoice_data.chain_id, first_meta.chain_id);
            
            println!(" Invoice: {}", invoice_data.invoice.client_order_id);
            println!("  Symbol: {}", invoice_data.invoice.symbol);
            println!("  Amount: {}", invoice_data.invoice.total_amount);
            println!("  Lots: {}", invoice_data.invoice.lots.len());
        }

        Ok(())
    }

    #[test]
    fn test_query_by_date_range() -> eyre::Result<()> {
        let file_path = PathBuf::from("persistence/InvoiceManager.json");
        
        if !file_path.exists() {
            println!("Skipping test - InvoiceManager.json not found");
            return Ok(());
        }

        let persistence = Arc::new(JsonFilePersistence::new(&file_path));
        let mut manager = MintInvoiceManager::new(persistence);
        manager.load()?;

        let from = chrono::DateTime::parse_from_rfc3339("2025-11-01T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let to = chrono::DateTime::parse_from_rfc3339("2025-12-01T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);

        let results = manager.get_invoices_in_date_range(from, to)?;

        println!("Found {} invoices in November 2025", results.len());
        assert!(results.len() >= 3);

        for invoice_data in &results {
            println!("  - {} ({}) at {}",
                invoice_data.client_order_id,
                invoice_data.symbol,
                invoice_data.timestamp
            );
        }

        Ok(())
    }

    #[test]
    fn test_get_specific_invoice() -> eyre::Result<()> {
        let file_path = PathBuf::from("persistence/InvoiceManager.json");
        
        if !file_path.exists() {
            println!("Skipping test - InvoiceManager.json not found");
            return Ok(());
        }

        let persistence = Arc::new(JsonFilePersistence::new(&file_path));
        let mut manager = MintInvoiceManager::new(persistence);
        manager.load()?;

        // Get known invoice
        let chain_id = 8453u32;
        let address: Address = "0xc0d3c9e530ca6d71469bb678e6592274154d9cad".parse()?;
        let client_order_id: ClientOrderId = "JYA-NIF-EYJ-9644".into();

        let result = manager.get_invoice(chain_id, address, &client_order_id)?;

        assert!(result.is_some(), "Should find invoice JYA-NIF-EYJ-9644");
        
        let invoice_data = result.unwrap();
        assert_eq!(invoice_data.invoice.symbol.to_string(), "SY100");
        assert_eq!(invoice_data.invoice.client_order_id, "JYA-NIF-EYJ-9644".into());
        
        println!(" Found invoice JYA-NIF-EYJ-9644");
        println!("  Payment ID: {}", invoice_data.invoice.payment_id);
        println!("  Lots: {}", invoice_data.invoice.lots.len());

        Ok(())
    }

    #[test]
    fn test_all_invoices_have_lots() -> eyre::Result<()> {
        let file_path = PathBuf::from("persistence/InvoiceManager.json");
        
        if !file_path.exists() {
            println!("Skipping test - InvoiceManager.json not found");
            return Ok(());
        }

        let persistence = Arc::new(JsonFilePersistence::new(&file_path));
        let mut manager = MintInvoiceManager::new(persistence);
        manager.load()?;

        // Get all invoices and verify they have lots
        for meta in manager.metadata.clone() {
            let result = manager.get_invoice(
                meta.chain_id,
                meta.address,
                &meta.client_order_id
            )?;
            
            if let Some(invoice_data) = result {
                assert!(invoice_data.invoice.lots.len() > 0, 
                    "Invoice {} should have lots", meta.client_order_id);
                
                println!("✓ {}: {} lots", 
                    invoice_data.invoice.client_order_id,
                    invoice_data.invoice.lots.len()
                );
            }
        }

        Ok(())
    }

    #[test]
    fn test_child_file_structure() -> eyre::Result<()> {
        let file_path = PathBuf::from("persistence/InvoiceManager.json");
        
        if !file_path.exists() {
            println!("Skipping test - InvoiceManager.json not found");
            return Ok(());
        }

        let persistence = Arc::new(JsonFilePersistence::new(&file_path));
        let mut manager = MintInvoiceManager::new(persistence);
        manager.load()?;

        // Verify child files exist
        if let Some(first_meta) = manager.metadata.first() {
            let key = MintInvoiceManager::create_invoice_key(
                first_meta.chain_id,
                &first_meta.address,
                &first_meta.client_order_id
            );
            
            // Expected path format
            let expected_path = format!("persistence/InvoiceManager/{}.json", key);
            println!("Expected child file: {}", expected_path);
            
            // Verify we can load it
            let result = manager.get_invoice(
                first_meta.chain_id,
                first_meta.address,
                &first_meta.client_order_id
            )?;
            
            assert!(result.is_some(), "Child file should exist and be loadable");
        }

        Ok(())
    }
}