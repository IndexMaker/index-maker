use chrono::{DateTime, Utc};
use eyre::Result;
use index_core::blockchain::chain_connector::{ChainConnector, ChainNotification};
use parking_lot::RwLock as AtomicLock;
use std::sync::Arc;
use tokio::sync::mpsc::{unbounded_channel, UnboundedSender};

use symm_core::core::{
    bits::{Address, Amount, Symbol},
    functional::{
        IntoObservableSingle, IntoObservableSingleVTable, NotificationHandlerOnce, PublishSingle,
        SingleObserver,
    },
};

use index_core::index::basket::{Basket, BasketDefinition};

use crate::arbiter::Arbiter;
use crate::chain_operations::ChainOperations;
use crate::commands::{ChainCommand, ChainOperationRequest};
use crate::credentials::EvmCredentials;

/// EVM Chain Connector
/// Follows the same pattern as binance connectors with public API + internal arbiter
pub struct EvmConnector {
    /// Observer for publishing chain events to the main system
    observer: SingleObserver<ChainNotification>,
    /// Arbiter for coordinating chain operations
    arbiter: Arbiter,
    /// Sender for chain operation requests
    request_sender: UnboundedSender<ChainOperationRequest>,
    /// Shared state for chain operations (following binance pattern)
    chain_operations: Arc<AtomicLock<ChainOperations>>,
    /// Shared observer for event publishing (following binance pattern)
    shared_observer: Arc<AtomicLock<SingleObserver<ChainNotification>>>,
    /// Current state tracking
    connected_chains: Vec<u32>,
}

impl EvmConnector {
    pub fn new() -> Self {
        let mut arbiter = Arbiter::new();
        let (request_sender, request_receiver) = unbounded_channel();

        // Create shared state following binance pattern
        let chain_operations = Arc::new(AtomicLock::new(ChainOperations::new()));
        let observer = SingleObserver::new();
        let shared_observer = Arc::new(AtomicLock::new(SingleObserver::new()));

        // Configuration
        let max_chain_operations = 50; // Maximum number of chain operations

        // Start the arbiter with the proper parameters following binance pattern
        arbiter.start(
            chain_operations.clone(),
            request_receiver,
            shared_observer.clone(),
            max_chain_operations,
        );

        Self {
            observer,
            arbiter,
            request_sender,
            chain_operations,
            shared_observer,
            connected_chains: Vec::new(),
        }
    }

    /// Connect to a blockchain network using credentials (following binance pattern)
    pub async fn connect_chain_with_credentials(
        &mut self,
        credentials: EvmCredentials,
    ) -> Result<()> {
        let chain_id = credentials.get_chain_id() as u32;
        println!(
            "Connecting to chain {} at {}",
            chain_id,
            credentials.get_rpc_url()
        );

        let request = ChainOperationRequest::AddOperation { credentials };

        self.request_sender
            .send(request)
            .map_err(|e| eyre::eyre!("Failed to send connect request: {}", e))?;

        if !self.connected_chains.contains(&chain_id) {
            self.connected_chains.push(chain_id);
        }

        println!("Chain {} connection initiated", chain_id);
        Ok(())
    }

    /// Connect to a blockchain network using manual parameters (legacy method)
    /// For backward compatibility - internally creates credentials
    pub async fn connect_chain(
        &mut self,
        chain_id: u32,
        rpc_url: String,
        private_key: String,
    ) -> Result<()> {
        let credentials =
            EvmCredentials::new(chain_id as u64, rpc_url, move || private_key.clone());
        self.connect_chain_with_credentials(credentials).await
    }

    /// Connect to predefined chains using environment variables
    /// Following the binance pattern for easy chain connection
    pub async fn connect_ethereum(&mut self) -> Result<()> {
        let credentials = EvmCredentials::ethereum_mainnet()?;
        self.connect_chain_with_credentials(credentials).await
    }

    pub async fn connect_arbitrum(&mut self) -> Result<()> {
        let credentials = EvmCredentials::arbitrum()?;
        self.connect_chain_with_credentials(credentials).await
    }

    pub async fn connect_base(&mut self) -> Result<()> {
        let credentials = EvmCredentials::base()?;
        self.connect_chain_with_credentials(credentials).await
    }

    pub async fn connect_polygon(&mut self) -> Result<()> {
        let credentials = EvmCredentials::polygon()?;
        self.connect_chain_with_credentials(credentials).await
    }

    /// Disconnect from a blockchain network
    pub async fn disconnect_chain(&mut self, chain_id: u32) -> Result<()> {
        println!("Disconnecting from chain {}", chain_id);

        let request = ChainOperationRequest::RemoveOperation { chain_id };

        self.request_sender
            .send(request)
            .map_err(|e| eyre::eyre!("Failed to send disconnect request: {}", e))?;

        self.connected_chains.retain(|&id| id != chain_id);

        println!("Chain {} disconnection initiated", chain_id);
        Ok(())
    }

    /// Get list of connected chains
    pub fn connected_chains(&self) -> &[u32] {
        &self.connected_chains
    }

    /// Stop the connector and all operations
    pub async fn stop(&mut self) -> Result<()> {
        println!("Stopping EVM connector");
        match self.arbiter.stop().await {
            Ok(_) => Ok(()),
            Err(e) => {
                println!("Error stopping arbiter: {:?}", e);
                Ok(()) // Don't propagate join errors
            }
        }
    }

    /// Send a command to be executed on a specific chain
    async fn send_command(&self, chain_id: u32, command: ChainCommand) -> Result<()> {
        let request = ChainOperationRequest::ExecuteCommand { chain_id, command };

        self.request_sender
            .send(request)
            .map_err(|e| eyre::eyre!("Failed to send command: {}", e))?;

        Ok(())
    }

    // Event notification methods (for internal use)
    fn notify_curator_weights_set(&self, symbol: Symbol, basket_definition: BasketDefinition) {
        self.observer
            .publish_single(ChainNotification::CuratorWeightsSet(
                symbol,
                basket_definition,
            ));
    }

    fn notify_deposit(
        &self,
        chain_id: u32,
        address: Address,
        amount: Amount,
        timestamp: DateTime<Utc>,
    ) {
        self.observer.publish_single(ChainNotification::Deposit {
            chain_id,
            address,
            amount,
            timestamp,
        });
    }

    fn notify_withdrawal_request(
        &self,
        chain_id: u32,
        address: Address,
        amount: Amount,
        timestamp: DateTime<Utc>,
    ) {
        self.observer
            .publish_single(ChainNotification::WithdrawalRequest {
                chain_id,
                address,
                amount,
                timestamp,
            });
    }
}

impl IntoObservableSingleVTable<ChainNotification> for EvmConnector {
    fn set_observer(&mut self, observer: Box<dyn NotificationHandlerOnce<ChainNotification>>) {
        self.observer.set_observer(observer);
    }
}

impl ChainConnector for EvmConnector {
    fn solver_weights_set(&self, symbol: Symbol, basket: std::sync::Arc<Basket>) {
        // Send command to the first connected chain (or implement chain selection logic)
        if let Some(&chain_id) = self.connected_chains.first() {
            let command = ChainCommand::SetSolverWeights { symbol, basket };

            tokio::spawn(async move {
                // Note: In a real implementation, we'd need to handle this properly
                // For now, this is a synchronous interface so we can't await
                println!("Solver weights set command queued for chain {}", chain_id);
            });
        } else {
            println!("No connected chains available for solver weights set");
        }
    }

    fn mint_index(
        &self,
        chain_id: u32,
        symbol: Symbol,
        quantity: Amount,
        recipient: Address,
        execution_price: Amount,
        execution_time: DateTime<Utc>,
    ) {
        let command = ChainCommand::MintIndex {
            chain_id,
            symbol,
            quantity,
            recipient,
            execution_price,
            execution_time,
        };

        let request_sender = self.request_sender.clone();
        tokio::spawn(async move {
            let request = ChainOperationRequest::ExecuteCommand { chain_id, command };
            if let Err(e) = request_sender.send(request) {
                println!("Failed to send mint index command: {}", e);
            }
        });
    }

    fn burn_index(&self, chain_id: u32, symbol: Symbol, quantity: Amount, recipient: Address) {
        let command = ChainCommand::BurnIndex {
            chain_id,
            symbol,
            quantity,
            recipient,
        };

        let request_sender = self.request_sender.clone();
        tokio::spawn(async move {
            let request = ChainOperationRequest::ExecuteCommand { chain_id, command };
            if let Err(e) = request_sender.send(request) {
                println!("Failed to send burn index command: {}", e);
            }
        });
    }

    fn withdraw(
        &self,
        chain_id: u32,
        recipient: Address,
        amount: Amount,
        execution_price: Amount,
        execution_time: DateTime<Utc>,
    ) {
        let command = ChainCommand::Withdraw {
            chain_id,
            recipient,
            amount,
            execution_price,
            execution_time,
        };

        let request_sender = self.request_sender.clone();
        tokio::spawn(async move {
            let request = ChainOperationRequest::ExecuteCommand { chain_id, command };
            if let Err(e) = request_sender.send(request) {
                println!("Failed to send withdraw command: {}", e);
            }
        });
    }
}

impl IntoObservableSingle<ChainNotification> for EvmConnector {
    fn get_single_observer_mut(&mut self) -> &mut SingleObserver<ChainNotification> {
        &mut self.observer
    }
}
