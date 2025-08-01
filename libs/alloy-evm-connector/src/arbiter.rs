use eyre::Result;
use symm_core::core::async_loop::AsyncLoop;
use tokio::sync::mpsc::UnboundedReceiver;
use itertools::Either;
use tokio::task::JoinError;
use std::sync::Arc;
use parking_lot::RwLock as AtomicLock;

use crate::chain_operations::ChainOperations;
use crate::commands::ChainOperationRequest;

/// Central arbiter for coordinating chain operations
/// Follows the exact pattern as binance modules' arbiters
pub struct Arbiter {
    /// Main async loop for the arbiter
    arbiter_loop: AsyncLoop<UnboundedReceiver<ChainOperationRequest>>,
}

impl Arbiter {
    /// Constructor - Initialize the arbiter
    pub fn new() -> Self {
        Self {
            arbiter_loop: AsyncLoop::new(),
        }
    }

    /// Start the main event loop
    /// Follows the exact binance pattern for parameter structure
    pub fn start(
        &mut self,
        chain_operations: Arc<AtomicLock<ChainOperations>>,      // State management
        mut operation_rx: UnboundedReceiver<ChainOperationRequest>, // Input channel
        max_chain_operations: usize,                            // Configuration
    ) {
        self.arbiter_loop.start(async move |cancel_token| {
            tracing::info!("Loop started");
            
            loop {
                tokio::select! {
                    _ = cancel_token.cancelled() => {
                        break;
                    }
                    Some(request) = operation_rx.recv() => {
                        
                        // Handle the request using ChainOperations methods
                        match ChainOperations::handle_chain_operation_request(
                            &chain_operations,
                            request,
                            max_chain_operations,
                        ).await {
                            Ok(()) => {
                                // Request handled successfully
                            }
                            Err(e) => {
                                tracing::error!("Failed to handle chain operation request: {}", e);
                            }
                        }
                    }
                }
            }

            // Cleanup: shutdown all operations following binance pattern
            let operations_to_stop = {
                let mut operations = chain_operations.write();
                operations.drain_all()
            };
            
            if let Err(err) = ChainOperations::stop_all(operations_to_stop).await {
                tracing::warn!("Error stopping chain operations {:?}", err);
            }
            
            tracing::info!("Loop exited");
            operation_rx
        });
    }


    /// Stop the arbiter and wait for completion
    /// Returns the input receiver following binance pattern
    pub async fn stop(&mut self) -> Result<UnboundedReceiver<ChainOperationRequest>, Either<JoinError, eyre::Report>> {
        self.arbiter_loop.stop().await
    }
}