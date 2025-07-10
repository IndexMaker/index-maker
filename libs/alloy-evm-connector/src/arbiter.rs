use eyre::Result;
use symm_core::core::async_loop::AsyncLoop;
use tokio::sync::mpsc::UnboundedReceiver;
use itertools::Either;
use tokio::task::JoinError;
use std::sync::Arc;
use parking_lot::RwLock as AtomicLock;
use symm_core::core::functional::{SingleObserver, PublishSingle};
use index_core::blockchain::chain_connector::ChainNotification;
use chrono::Utc;

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
        observer: Arc<AtomicLock<SingleObserver<ChainNotification>>>, // Event publisher
        max_chain_operations: usize,                            // Configuration
    ) {
        todo!();
        // self.arbiter_loop.start(async move |cancel_token| {
        //     println!("Chain operations arbiter started");
            
        //     loop {
        //         tokio::select! {
        //             _ = cancel_token.cancelled() => {
        //                 println!("Chain operations arbiter cancelled");
        //                 break;
        //             }
        //             Some(request) = operation_rx.recv() => {
        //                 println!("Arbiter received request: {:?}", std::mem::discriminant(&request));
                        
        //                 // Handle the request using the state management pattern
        //                 match Self::handle_chain_operation_request(
        //                     &chain_operations,
        //                     request,
        //                     &observer,
        //                     max_chain_operations,
        //                 ).await {
        //                     Ok(()) => {
        //                         // Request handled successfully
        //                     }
        //                     Err(e) => {
        //                         eprintln!("Failed to handle chain operation request: {}", e);
        //                     }
        //                 }
        //             }
        //         }
        //     }

        //     // Cleanup: shutdown all operations (following binance pattern)
        //     {
        //         let mut operations = chain_operations.write();
        //         drop(operations); // Release the lock before async operation
        //         // Note: In a real implementation, we'd need a different approach for cleanup
        //         println!("Chain operations cleanup completed");
        //     }

        //     println!("Chain operations arbiter stopped");
        //     operation_rx
        // });
    }

    /// Handle chain operation request (following binance pattern)
    async fn handle_chain_operation_request(
        chain_operations: &Arc<AtomicLock<ChainOperations>>,
        request: ChainOperationRequest,
        observer: &Arc<AtomicLock<SingleObserver<ChainNotification>>>,
        max_operations: usize,
    ) -> Result<()> {
        let mut operations = chain_operations.write();
        
        match request {
            ChainOperationRequest::AddOperation { credentials } => {
                let chain_id = credentials.get_chain_id() as u32;
                
                // Check if we've reached the maximum number of operations
                if operations.operation_count() >= max_operations {
                    eprintln!("Maximum number of chain operations ({}) reached", max_operations);
                    return Ok(());
                }

                // Add the new chain operation using credentials (following binance pattern)
                operations.add_operation_with_credentials(credentials).await?;
                
                // Notify via observer (following binance pattern)
                {
                    let obs = observer.read();
                    obs.publish_single(ChainNotification::Deposit {
                        chain_id,
                        address: Default::default(), // placeholder
                        amount: Default::default(),  // placeholder
                        timestamp: Utc::now(),
                    });
                }
                
                println!("Added chain operation for chain {}", chain_id);
            }
            
            ChainOperationRequest::RemoveOperation { chain_id } => {
                // Remove the chain operation
                operations.remove_operation(chain_id).await?;
                
                // Notify via observer
                {
                    let obs = observer.read();
                    obs.publish_single(ChainNotification::WithdrawalRequest {
                        chain_id,
                        address: Default::default(), // placeholder
                        amount: Default::default(),  // placeholder
                        timestamp: Utc::now(),
                    });
                }
                
                println!("Removed chain operation for chain {}", chain_id);
            }
            
            ChainOperationRequest::ExecuteCommand { chain_id, command } => {
                // Execute the command on the specified chain
                operations.handle_request(ChainOperationRequest::ExecuteCommand { 
                    chain_id, 
                    command 
                }).await?;
            }
        }

        Ok(())
    }

    /// Stop the arbiter and wait for completion
    /// Returns the input receiver following binance pattern
    pub async fn stop(&mut self) -> Result<UnboundedReceiver<ChainOperationRequest>, Either<JoinError, eyre::Report>> {
        println!("Stopping chain operations arbiter");
        self.arbiter_loop.stop().await
    }
}