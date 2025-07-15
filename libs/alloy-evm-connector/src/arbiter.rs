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
        operation_rx: UnboundedReceiver<ChainOperationRequest>, // Input channel
        observer: Arc<AtomicLock<SingleObserver<ChainNotification>>>, // Event publisher
        max_chain_operations: usize,                            // Configuration
    ) {
        self.arbiter_loop.start(async move |cancel_token| {
            println!("Chain operations arbiter started");
            
            let mut operation_rx = operation_rx; // Make it mutable in the async block
            
            loop {
                tokio::select! {
                    _ = cancel_token.cancelled() => {
                        println!("Chain operations arbiter cancelled");
                        break;
                    }
                    Some(request) = operation_rx.recv() => {
                        println!("Arbiter received request: {:?}", std::mem::discriminant(&request));
                        
                        // Handle the request using the state management pattern
                        match Self::handle_chain_operation_request(
                            &chain_operations,
                            request,
                            &observer,
                            max_chain_operations,
                        ).await {
                            Ok(()) => {
                                // Request handled successfully
                            }
                            Err(e) => {
                                eprintln!("Failed to handle chain operation request: {}", e);
                            }
                        }
                    }
                }
            }

            // Cleanup: shutdown all operations (following binance pattern)
            {
                let operations = chain_operations.read();
                drop(operations); // Release the lock before async operation
                println!("Chain operations cleanup completed");
            }

            println!("Chain operations arbiter stopped");
            operation_rx
        });
    }

    /// Handle chain operation request (following binance pattern)
    async fn handle_chain_operation_request(
        chain_operations: &Arc<AtomicLock<ChainOperations>>,
        request: ChainOperationRequest,
        observer: &Arc<AtomicLock<SingleObserver<ChainNotification>>>,
        max_operations: usize,
    ) -> Result<()> {
        match request {
            ChainOperationRequest::AddOperation { credentials } => {
                let chain_id = credentials.get_chain_id() as u32;
                
                // Check if we've reached the maximum number of operations
                let can_add = {
                    let operations = chain_operations.read();
                    operations.operation_count() < max_operations
                };
                
                if !can_add {
                    eprintln!("Maximum number of chain operations ({}) reached", max_operations);
                    return Ok(());
                }

                // Add the new chain operation using credentials (following binance pattern)
                // This is synchronous so no Arc cloning needed
                {
                    let mut operations = chain_operations.write();
                    operations.add_operation_with_credentials(credentials)?;
                }
                
                // Notify via observer (following binance pattern)
                {
                    let obs = observer.read();
                    obs.publish_single(ChainNotification::ChainConnected {
                        chain_id,
                        timestamp: Utc::now(),
                    });
                }
                
                println!("Added chain operation for chain {}", chain_id);
            }
            
            ChainOperationRequest::RemoveOperation { chain_id } => {
                // Following senior dev's advice: extract operation while holding lock,
                // then call async method on cloned Arc outside the lock
                let operation_to_stop = {
                    let mut operations = chain_operations.write();
                    operations.remove_operation_sync(chain_id)
                };
                
                // Clone the observer Arc before async operation
                let observer_clone = observer.clone();
                
                // Handle async operation outside the lock
                if let Some(mut operation) = operation_to_stop {
                    match operation.stop().await {
                        Ok(()) => {
                            // Notify via observer
                            let obs = observer_clone.read();
                            obs.publish_single(ChainNotification::ChainDisconnected {
                                chain_id,
                                timestamp: Utc::now(),
                            });
                            println!("Removed chain operation for chain {}", chain_id);
                        }
                        Err(e) => {
                            eprintln!("Failed to stop chain operation for chain {}: {}", chain_id, e);
                        }
                    }
                } else {
                    // Notify via observer anyway (operation was not found)
                    let obs = observer_clone.read();
                    obs.publish_single(ChainNotification::ChainDisconnected {
                        chain_id,
                        timestamp: Utc::now(),
                    });
                    println!("Chain operation for chain {} not found", chain_id);
                }
            }
            
            ChainOperationRequest::ExecuteCommand { chain_id, command } => {
                // ExecuteCommand is no longer handled by Arbiter
                // Commands should be sent directly to chain_operations
                eprintln!("ExecuteCommand should not be sent to Arbiter. Use direct chain_operations.send_command() instead.");
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