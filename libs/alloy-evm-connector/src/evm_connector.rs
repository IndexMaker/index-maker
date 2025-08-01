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

use crate::across_bridge::AcrossCollateralBridge;
use crate::arbiter::Arbiter;
use crate::chain_operations::ChainOperations;
use crate::commands::{ChainCommand, ChainOperationRequest};
use crate::credentials::EvmCredentials;
use crate::designation::EvmCollateralDesignation;
use crate::designation_details::EvmDesignationDetails;
use crate::erc20_bridge::Erc20CollateralBridge;
use index_core::collateral::collateral_router::{CollateralBridge, CollateralDesignation};

/// EVM Chain Connector
/// Follows the same pattern as binance connectors with public API + internal arbiter
pub struct EvmConnector {
    /// Observer for publishing chain events to the main system
    observer: SingleObserver<ChainNotification>,
    /// Arbiter for coordinating chain operations (owned by EvmConnector)
    arbiter: Option<Arbiter>,
    /// Sender for chain operation requests
    request_sender: Option<UnboundedSender<ChainOperationRequest>>,
    /// Shared state for chain operations (owned by EvmConnector)
    chain_operations: Arc<AtomicLock<ChainOperations>>,
    /// Shared observer for event publishing (following binance pattern)
    shared_observer: Arc<AtomicLock<SingleObserver<ChainNotification>>>,
    /// Current state tracking
    connected_chains: Vec<u32>,
}

impl EvmConnector {
    pub fn new() -> Self {
        // Create shared state following binance pattern
        let observer = SingleObserver::new();
        let shared_observer = Arc::new(AtomicLock::new(SingleObserver::new()));
        let chain_operations = Arc::new(AtomicLock::new(ChainOperations::new(
            shared_observer.clone(),
        )));

        Self {
            observer,
            arbiter: None,
            request_sender: None,
            chain_operations,
            shared_observer,
            connected_chains: Vec::new(),
        }
    }

    /// Start the EVM connector and initialize the arbiter
    pub fn start(&mut self) -> Result<()> {
        let mut arbiter = Arbiter::new();
        let (request_sender, request_receiver) = unbounded_channel();

        // Configuration
        let max_chain_operations = 50; // Maximum number of chain operations

        // Start the arbiter with the proper parameters following binance pattern
        arbiter.start(
            self.chain_operations.clone(),
            request_receiver,
            max_chain_operations,
        );

        self.arbiter = Some(arbiter);
        self.request_sender = Some(request_sender);

        tracing::info!("EVM Connector started with arbiter");
        Ok(())
    }

    /// Connect to a blockchain network using credentials (following binance pattern)
    pub async fn connect_chain_with_credentials(
        &mut self,
        credentials: EvmCredentials,
    ) -> Result<()> {
        let chain_id = credentials.get_chain_id() as u32;

        let request = ChainOperationRequest::AddOperation { credentials };

        if let Some(sender) = &self.request_sender {
            sender
                .send(request)
                .map_err(|e| eyre::eyre!("Failed to send connect request: {}", e))?;
        } else {
            return Err(eyre::eyre!(
                "EVM Connector not started. Call start() first."
            ));
        }

        if !self.connected_chains.contains(&chain_id) {
            self.connected_chains.push(chain_id);
        }

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
        let credentials = EvmCredentials::new(chain_id, rpc_url, move || private_key.clone());
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

    /// Disconnect from a blockchain network
    pub async fn disconnect_chain(&mut self, chain_id: u32) -> Result<()> {
        let request = ChainOperationRequest::RemoveOperation { chain_id };

        if let Some(sender) = &self.request_sender {
            sender
                .send(request)
                .map_err(|e| eyre::eyre!("Failed to send disconnect request: {}", e))?;
        } else {
            return Err(eyre::eyre!(
                "EVM Connector not started. Call start() first."
            ));
        }

        self.connected_chains.retain(|&id| id != chain_id);

        Ok(())
    }

    /// Get list of connected chains
    pub fn connected_chains(&self) -> &[u32] {
        &self.connected_chains
    }

    /// Generic bridge creation method - automatically selects bridge type based on designations
    pub fn create_bridge(
        &self,
        source: Arc<std::sync::RwLock<EvmCollateralDesignation>>,
        destination: Arc<std::sync::RwLock<EvmCollateralDesignation>>,
    ) -> Arc<std::sync::RwLock<dyn CollateralBridge>> {
        let source_name = source.read().unwrap().get_name();
        let destination_name = destination.read().unwrap().get_name();

        // Determine bridge type based on cross-chain check
        let is_cross_chain = source
            .read()
            .unwrap()
            .is_cross_chain(&*destination.read().unwrap());

        if is_cross_chain {
            // Cross-chain transfer - use Across bridge
            tracing::info!(
                "Creating Across bridge for cross-chain transfer: {} -> {}",
                source_name,
                destination_name
            );

            let bridge = AcrossCollateralBridge::new_with_shared_operations(
                source,
                destination,
                self.chain_operations.clone(),
            );

            bridge as Arc<std::sync::RwLock<dyn CollateralBridge>>
        } else {
            // Same-chain transfer - use ERC20 bridge
            let bridge = Erc20CollateralBridge::new_with_shared_operations(
                source,
                destination,
                self.chain_operations.clone(),
            );

            bridge as Arc<std::sync::RwLock<dyn CollateralBridge>>
        }
    }

    /// Create a new AcrossCollateralBridge with shared chain_operations (legacy method)
    pub fn create_across_bridge(
        &self,
        source: Arc<std::sync::RwLock<EvmCollateralDesignation>>,
        destination: Arc<std::sync::RwLock<EvmCollateralDesignation>>,
    ) -> Arc<std::sync::RwLock<AcrossCollateralBridge>> {
        AcrossCollateralBridge::new_with_shared_operations(
            source,
            destination,
            self.chain_operations.clone(),
        )
    }

    /// Create a new Erc20CollateralBridge with shared chain_operations (legacy method)
    pub fn create_erc20_bridge(
        &self,
        source: Arc<std::sync::RwLock<EvmCollateralDesignation>>,
        destination: Arc<std::sync::RwLock<EvmCollateralDesignation>>,
    ) -> Arc<std::sync::RwLock<Erc20CollateralBridge>> {
        Erc20CollateralBridge::new_with_shared_operations(
            source,
            destination,
            self.chain_operations.clone(),
        )
    }

    /// Stop the connector and all operations
    pub async fn stop(&mut self) -> Result<()> {
        tracing::info!("Stopping EVM connector");
        if let Some(mut arbiter) = self.arbiter.take() {
            match arbiter.stop().await {
                Ok(_) => Ok(()),
                Err(e) => {
                    tracing::error!("Error stopping arbiter: {:?}", e);
                    Ok(()) // Don't propagate join errors
                }
            }
        } else {
            Ok(())
        }
    }

    /// Send a command to be executed on a specific chain (direct access to chain_operations)
    pub fn send_command(&self, chain_id: u32, command: ChainCommand) -> Result<()> {
        let operations = self.chain_operations.read();
        operations.execute_command(chain_id, command)?;
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
        // Sadhbh: You probably want to remove those notify_XXX functions, because you
        // probably want to publish those events in places where you handle the commands.
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
    fn solver_weights_set(&self, symbol: Symbol, _basket: std::sync::Arc<Basket>) {
        // Send command to the first connected chain (or implement chain selection logic)
        if let Some(&chain_id) = self.connected_chains.first() {
            // Sadhbh: I made send_command sync, so it can be just called like that
            // SetSolverWeights command removed per sonia's feedback
            // Only keeping the needed commands: ExecuteCompleteAcrossDeposit, Erc20Transfer, MintIndex, BurnIndex, Withdraw
            tracing::info!(
                "Solver weights set for symbol {:?} on chain {} (command removed)",
                symbol,
                chain_id
            );
        } else {
            tracing::warn!("No connected chains available for solver weights set");
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

        if let Err(e) = self.send_command(chain_id, command) {
            tracing::error!("Failed to send mint index command: {}", e);
        }
    }

    fn burn_index(&self, chain_id: u32, symbol: Symbol, quantity: Amount, recipient: Address) {
        let command = ChainCommand::BurnIndex {
            chain_id,
            symbol,
            quantity,
            recipient,
        };

        if let Err(e) = self.send_command(chain_id, command) {
            tracing::error!("Failed to send burn index command: {}", e);
        }
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

        if let Err(e) = self.send_command(chain_id, command) {
            tracing::error!("Failed to send withdraw command: {}", e);
        }
    }
}

impl IntoObservableSingle<ChainNotification> for EvmConnector {
    fn get_single_observer_mut(&mut self) -> &mut SingleObserver<ChainNotification> {
        &mut self.observer
    }
}
