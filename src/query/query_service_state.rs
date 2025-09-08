use std::sync::{Arc, RwLock as ComponentLock};

use parking_lot::RwLock;
use symm_core::order_sender::inventory_manager::InventoryManager;

use crate::{
    collateral::collateral_manager::CollateralManager,
    solver::{index_order_manager::IndexOrderManager, mint_invoice_manager::MintInvoiceManager},
};

pub struct QueryServiceState {
    collateral_manager: Arc<ComponentLock<CollateralManager>>,
    index_order_manager: Arc<ComponentLock<IndexOrderManager>>,
    inventory_manager: Arc<RwLock<InventoryManager>>,
    invoice_manager: Arc<RwLock<MintInvoiceManager>>,
}

impl QueryServiceState {
    pub fn new(
        collateral_manager: Arc<ComponentLock<CollateralManager>>,
        index_order_manager: Arc<ComponentLock<IndexOrderManager>>,
        inventory_manager: Arc<RwLock<InventoryManager>>,
        invoice_manager: Arc<RwLock<MintInvoiceManager>>,
    ) -> Self {
        Self {
            collateral_manager,
            index_order_manager,
            inventory_manager,
            invoice_manager,
        }
    }

    pub fn get_collateral_manager(&self) -> &Arc<ComponentLock<CollateralManager>> {
        &self.collateral_manager
    }

    pub fn get_index_order_manager(&self) -> &Arc<ComponentLock<IndexOrderManager>> {
        &self.index_order_manager
    }

    pub fn get_inventory_manager(&self) -> &Arc<RwLock<InventoryManager>> {
        &self.inventory_manager
    }

    pub fn get_invoice_manager(&self) -> &Arc<RwLock<MintInvoiceManager>> {
        &self.invoice_manager
    }
}
