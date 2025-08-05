use eyre::Result;
use futures_util::future::join_all;
use itertools::Itertools;
use parking_lot::RwLock as AtomicLock;
use std::collections::HashMap;
use std::sync::Arc;
use symm_core::core::functional::SingleObserver;
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};

use crate::chain_operation::ChainOperation;
use crate::commands::{ChainCommand, ChainOperationRequest, ChainOperationResult};
use crate::credentials::EvmCredentials;
use index_core::blockchain::chain_connector::ChainNotification;

/// Chain operations manager
/// Manages a pool of chain operation workers, one per blockchain
pub struct ChainOperations {
    /// Active chain operations (chain_id -> operation)
    operations: HashMap<u32, ChainOperation>,
    /// Channels for sending commands to specific chains
    command_senders: HashMap<u32, UnboundedSender<ChainCommand>>,
    /// Channel for receiving operation results
    result_receiver: Option<UnboundedReceiver<ChainOperationResult>>,
    /// Sender for operation results (shared among all workers)
    result_sender: UnboundedSender<ChainOperationResult>,
    /// Observer for chain notifications (shared among all workers)
    chain_observer: Arc<AtomicLock<SingleObserver<ChainNotification>>>,
}

impl ChainOperations {
    pub fn new(chain_observer: Arc<AtomicLock<SingleObserver<ChainNotification>>>) -> Self {
        let (result_sender, result_receiver) = unbounded_channel();

        Self {
            operations: HashMap::new(),
            command_senders: HashMap::new(),
            result_receiver: Some(result_receiver),
            result_sender,
            chain_observer,
        }
    }

    /// Take the result receiver (can only be called once)
    pub fn take_result_receiver(&mut self) -> Option<UnboundedReceiver<ChainOperationResult>> {
        self.result_receiver.take()
    }

    /// Get the count of active operations (following binance pattern)
    pub fn operation_count(&self) -> usize {
        self.operations.len()
    }

    /// Add a new chain operation using credentials (following binance pattern)
    pub fn add_operation_with_credentials(&mut self, credentials: EvmCredentials) -> Result<()> {
        let chain_id = credentials.get_chain_id() as u32;
        let rpc_url = credentials.get_rpc_url();
        let private_key = credentials.get_private_key();

        self.add_operation(chain_id, rpc_url, private_key)
    }

    /// Process a chain operation request
    pub async fn handle_request(&mut self, request: ChainOperationRequest) -> Result<()> {
        match request {
            ChainOperationRequest::AddOperation { credentials } => {
                self.add_operation_with_credentials(credentials)?;
            }
            ChainOperationRequest::RemoveOperation { chain_id } => {
                if let Some(mut operation) = self.remove_operation(chain_id) {
                    operation.stop().await?;
                }
            }
            ChainOperationRequest::ExecuteCommand { chain_id, command } => {
                self.execute_command(chain_id, command)?;
            }
        }
        Ok(())
    }

    /// Add a new chain operation
    pub fn add_operation(
        &mut self,
        chain_id: u32,
        rpc_url: String,
        private_key: String,
    ) -> Result<()> {
        if self.operations.contains_key(&chain_id) {
            tracing::warn!("Chain operation for chain {} already exists", chain_id);
            return Ok(());
        }

        // Create command channel for this chain
        let (command_sender, command_receiver) = unbounded_channel();

        // Create and start the chain operation
        let mut operation = ChainOperation::new(
            chain_id,
            rpc_url,
            private_key,
            self.result_sender.clone(),
            self.chain_observer.clone(),
        );

        operation.start(command_receiver)?;

        // Store the operation and command sender
        self.operations.insert(chain_id, operation);
        self.command_senders.insert(chain_id, command_sender);

        Ok(())
    }

    /// Remove a chain operation, returns it so user can call stop() on it
    pub fn remove_operation(&mut self, chain_id: u32) -> Option<ChainOperation> {
        // Remove command sender first
        self.command_senders.remove(&chain_id);

        // Remove the operation (returns it so user can stop it outside the lock)
        self.operations.remove(&chain_id)
    }

    /// Execute a command on a specific chain
    pub fn execute_command(&self, chain_id: u32, command: ChainCommand) -> Result<()> {
        if let Some(sender) = self.command_senders.get(&chain_id) {
            sender
                .send(command)
                .map_err(|e| eyre::eyre!("Failed to send command to chain {}: {}", chain_id, e))?;
        } else {
            return Err(eyre::eyre!(
                "No active operation found for chain {}",
                chain_id
            ));
        }
        Ok(())
    }

    /// Get list of connected chain IDs
    pub fn connected_chain_ids(&self) -> Vec<u32> {
        self.operations.keys().copied().collect()
    }

    /// Check if a chain operation is connected
    pub fn is_connected(&self, chain_id: u32) -> bool {
        self.operations.contains_key(&chain_id)
    }

    /// Drain all operations, returns them so user can call stop() on each
    pub fn drain_all(&mut self) -> Vec<ChainOperation> {
        let _ = self.command_senders.drain().count();
        self.operations.drain().map(|(_, v)| v).collect_vec()
    }

    /// Static method to stop all operations (following binance pattern)
    pub async fn stop_all(mut operations: Vec<ChainOperation>) -> Result<()> {
        let stop_futures = operations.iter_mut().map(|v| v.stop()).collect_vec();

        let (_, failures): (Vec<_>, Vec<_>) =
            join_all(stop_futures).await.into_iter().partition_result();

        if !failures.is_empty() {
            Err(eyre::eyre!(
                "Chain operations join failed {}",
                failures.iter().map(|e| format!("{:?}", e)).join(";"),
            ))?
        }

        Ok(())
    }

    /// Handle chain operation request (moved from Arbiter, following binance pattern)
    pub async fn handle_chain_operation_request(
        chain_operations: &Arc<AtomicLock<ChainOperations>>,
        request: ChainOperationRequest,
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
                    tracing::error!(
                        "Maximum number of chain operations ({}) reached",
                        max_operations
                    );
                    return Ok(());
                }

                // Add the new chain operation using credentials (following binance pattern)
                // This is synchronous so no Arc cloning needed
                {
                    let mut operations = chain_operations.write();
                    operations.add_operation_with_credentials(credentials)?;
                }

                // Note: Connected event is now emitted by ChainOperation itself
            }

            ChainOperationRequest::RemoveOperation { chain_id } => {
                // Following sonia's advice: extract operation while holding lock,
                // then call async method on cloned Arc outside the lock
                let operation_to_stop = {
                    let mut operations = chain_operations.write();
                    operations.remove_operation(chain_id)
                };

                // Handle async operation outside the lock
                if let Some(mut operation) = operation_to_stop {
                    if let Err(e) = operation.stop().await {
                        tracing::error!(
                            "Failed to stop chain operation for chain {}: {}",
                            chain_id,
                            e
                        );
                    }
                    // Note: Disconnected event is now emitted by ChainOperation itself
                } else {
                    tracing::warn!("Chain operation for chain {} not found", chain_id);
                }
            }

            ChainOperationRequest::ExecuteCommand {
                chain_id: _,
                command: _,
            } => {
                // ExecuteCommand is no longer handled by Arbiter
                // Commands should be sent directly to chain_operations
                tracing::error!("ExecuteCommand should not be sent to Arbiter. Use direct chain_operations.send_command() instead.");
            }
        }

        Ok(())
    }
}

impl Drop for ChainOperations {
    fn drop(&mut self) {
        if !self.operations.is_empty() {
            tracing::error!(
                "ChainOperations dropped with {} active operations",
                self.operations.len()
            );
        }
    }
}
