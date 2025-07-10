use eyre::Result;
use std::collections::HashMap;
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};

use crate::chain_operation::ChainOperation;
use crate::commands::{ChainCommand, ChainOperationRequest, ChainOperationResult};
use crate::credentials::EvmCredentials;

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
}

impl ChainOperations {
    pub fn new() -> Self {
        let (result_sender, result_receiver) = unbounded_channel();

        Self {
            operations: HashMap::new(),
            command_senders: HashMap::new(),
            result_receiver: Some(result_receiver),
            result_sender,
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
    pub async fn add_operation_with_credentials(&mut self, credentials: EvmCredentials) -> Result<()> {
        let chain_id = credentials.get_chain_id() as u32;
        let rpc_url = credentials.get_rpc_url();
        let private_key = credentials.get_private_key();
        
        self.add_operation(chain_id, rpc_url, private_key).await
    }

    /// Process a chain operation request
    pub async fn handle_request(&mut self, request: ChainOperationRequest) -> Result<()> {
        match request {
            ChainOperationRequest::AddOperation { credentials } => {
                self.add_operation_with_credentials(credentials).await?;
            }
            ChainOperationRequest::RemoveOperation { chain_id } => {
                self.remove_operation(chain_id).await?;
            }
            ChainOperationRequest::ExecuteCommand { chain_id, command } => {
                self.execute_command(chain_id, command).await?;
            }
        }
        Ok(())
    }

    /// Add a new chain operation
    pub async fn add_operation(
        &mut self,
        chain_id: u32,
        rpc_url: String,
        private_key: String,
    ) -> Result<()> {
        if self.operations.contains_key(&chain_id) {
            println!("Chain operation for chain {} already exists", chain_id);
            return Ok(());
        }

        println!("Adding chain operation for chain {}", chain_id);

        // Create command channel for this chain
        let (command_sender, command_receiver) = unbounded_channel();

        // Create and start the chain operation
        let mut operation = ChainOperation::new(
            chain_id,
            rpc_url,
            private_key,
            self.result_sender.clone(),
        );

        operation.start(command_receiver)?;

        // Store the operation and command sender
        self.operations.insert(chain_id, operation);
        self.command_senders.insert(chain_id, command_sender);

        println!("Chain operation for chain {} started successfully", chain_id);
        Ok(())
    }

    /// Remove a chain operation
    pub async fn remove_operation(&mut self, chain_id: u32) -> Result<()> {
        println!("Removing chain operation for chain {}", chain_id);

        // Remove command sender first
        self.command_senders.remove(&chain_id);

        // Stop and remove the operation
        if let Some(mut operation) = self.operations.remove(&chain_id) {
            operation.stop().await?;
            println!("Chain operation for chain {} stopped successfully", chain_id);
        } else {
            println!("Chain operation for chain {} not found", chain_id);
        }

        Ok(())
    }

    /// Execute a command on a specific chain
    async fn execute_command(&self, chain_id: u32, command: ChainCommand) -> Result<()> {
        if let Some(sender) = self.command_senders.get(&chain_id) {
            sender.send(command).map_err(|e| {
                eyre::eyre!("Failed to send command to chain {}: {}", chain_id, e)
            })?;
            println!("Command sent to chain {}", chain_id);
        } else {
            return Err(eyre::eyre!(
                "No active operation found for chain {}",
                chain_id
            ));
        }
        Ok(())
    }

    /// Get list of active chain IDs
    pub fn active_chains(&self) -> Vec<u32> {
        self.operations.keys().copied().collect()
    }

    /// Check if a chain operation is active
    pub fn is_active(&self, chain_id: u32) -> bool {
        self.operations.contains_key(&chain_id)
    }

    /// Shutdown all operations
    pub async fn shutdown(&mut self) -> Result<()> {
        println!("Shutting down all chain operations");

        let chain_ids: Vec<u32> = self.operations.keys().copied().collect();
        for chain_id in chain_ids {
            self.remove_operation(chain_id).await?;
        }

        println!("All chain operations shut down");
        Ok(())
    }
}

impl Drop for ChainOperations {
    fn drop(&mut self) {
        if !self.operations.is_empty() {
            println!(
                "ChainOperations dropped with {} active operations",
                self.operations.len()
            );
        }
    }
}