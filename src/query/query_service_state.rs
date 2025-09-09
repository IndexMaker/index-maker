use std::sync::{Arc, RwLock as ComponentLock};

use chrono::{Duration, Utc};
use parking_lot::RwLock;
use symm_core::{
    core::limit::Limiter,
    order_sender::inventory_manager::{
        GetReconciledPositionsResponse, InventoryManager, InventoryManagerSnapshot,
    },
};
use tokio::time::sleep;

use crate::{
    collateral::collateral_manager::CollateralManager,
    solver::{index_order_manager::IndexOrderManager, mint_invoice_manager::MintInvoiceManager},
};

pub struct QueryServiceState {
    collateral_manager: Arc<ComponentLock<CollateralManager>>,
    index_order_manager: Arc<ComponentLock<IndexOrderManager>>,
    inventory_manager: Arc<RwLock<InventoryManager>>,
    invoice_manager: Arc<RwLock<MintInvoiceManager>>,
    inventory_snapshot: Arc<InventoryManagerSnapshot>,
    request_limiter: RwLock<Limiter>,
}

impl QueryServiceState {
    pub fn new(
        collateral_manager: Arc<ComponentLock<CollateralManager>>,
        index_order_manager: Arc<ComponentLock<IndexOrderManager>>,
        inventory_manager: Arc<RwLock<InventoryManager>>,
        invoice_manager: Arc<RwLock<MintInvoiceManager>>,
        inventory_snapshot: Arc<InventoryManagerSnapshot>,
    ) -> Self {
        Self {
            collateral_manager,
            index_order_manager,
            inventory_manager,
            invoice_manager,
            inventory_snapshot,
            request_limiter: RwLock::new(Limiter::new(100, Duration::seconds(10))),
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

    pub fn get_inventory_snapshot(&self) -> Arc<GetReconciledPositionsResponse> {
        // We could call InventoryManager::get_reconciled_positions() and pass
        // in oneshot send() as callback, and then await on that oneshot.
        // Problem is that we don't want to block on obtaining read-lock on
        // InventoryManager, so instead InventoryManager itself manages a
        // snapshot we can request update by calling
        // InventoryManager::update_snapshot().  We could make it take oneshot
        // callback, and we could await for update to complete, but the idea is
        // that we don't. We use RwLock::try_lock() so that when there is no
        // contention on InventoryManager we would get it to update the
        // snapshot. If there is contention, then there is batch currently
        // active, and we would be blocked until batch completes, because of
        // high amount of contention. We want query service to be responsive and
        // eventually consistent, so in idle periods we want to refresh the
        // snapshot and in busy periods we just return most recent snapshot,
        // which would be updated after every mint and also when last batch
        // isn't continued.  This gives sensible amount of refreshes, yet
        // prevents spamming.
        if let Some(read_guard) = self.inventory_manager.try_read() {
            if let Err(err) = read_guard.update_snapshot() {
                tracing::warn!("Failed to update snapshot: {:?}", err);
            }
        }
        self.inventory_snapshot.get()
    }

    pub async fn will_handle_request(&self) -> eyre::Result<()> {
        if !self.request_limiter.write().try_consume(1, Utc::now()) {
            let sleep_time = self
                .request_limiter
                .read()
                .waiting_period_half_limit(Utc::now())
                .as_seconds_f64();

            tracing::info!("Rate limit reached. Must wait for: {}s", sleep_time);

            sleep(std::time::Duration::from_secs_f64(sleep_time)).await;

            if !self.request_limiter.write().try_consume(1, Utc::now()) {
                Err(eyre::eyre!("Failed to satisfy rate-limit"))?
            }
        }
        Ok(())
    }
}
