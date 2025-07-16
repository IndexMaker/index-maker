use std::sync::Arc;

use crate::credentials::EvmCredentials;
use crate::custody_helper::Party;
use alloy::primitives::FixedBytes;
use chrono::{DateTime, Utc};
use index_core::blockchain::chain_connector::ChainNotification;
use index_core::collateral::collateral_router::CollateralRouterEvent;
use index_core::index::basket::Basket;
use parking_lot::RwLock as AtomicLock;
use symm_core::core::bits::{Address, Amount, ClientOrderId, Symbol};
use symm_core::core::functional::SingleObserver;

/// Commands that can be sent to the chain operations arbiter
#[derive(Clone)]
pub enum ChainCommand {
    /// Simple ERC20 transfer (wallet to wallet on same chain)
    Erc20Transfer {
        chain_id: u32,
        token_address: Address,
        from: Address,
        to: Address,
        amount: Amount,
        callback: Arc<dyn Fn(Amount, Amount) -> eyre::Result<()> + Send + Sync>,
    },
    /// Mint index tokens
    MintIndex {
        chain_id: u32,
        symbol: Symbol,
        quantity: Amount,
        recipient: Address,
        execution_price: Amount,
        execution_time: DateTime<Utc>,
    },
    /// Burn index tokens
    BurnIndex {
        chain_id: u32,
        symbol: Symbol,
        quantity: Amount,
        recipient: Address,
    },
    /// Withdraw funds
    Withdraw {
        chain_id: u32,
        recipient: Address,
        amount: Amount,
        execution_price: Amount,
        execution_time: DateTime<Utc>,
    },
    /// Execute complete Across deposit flow (all 12 steps)
    ExecuteCompleteAcrossDeposit {
        chain_id: u32,
        recipient: Address,
        input_token: Address,
        output_token: Address,
        deposit_amount: Amount,
        origin_chain_id: u64,
        destination_chain_id: u64,
        party: Party,
        callback: Arc<dyn Fn(Amount, Amount) -> eyre::Result<()> + Send + Sync>,
    },
}

/// Requests for managing chain operations
/// Following the binance pattern with credential-based requests
pub enum ChainOperationRequest {
    /// Add a new chain operation using credentials (following binance pattern)
    AddOperation { credentials: EvmCredentials },
    /// Remove a chain operation
    RemoveOperation { chain_id: u32 },
    /// Execute a command on a specific chain
    ExecuteCommand {
        chain_id: u32,
        command: ChainCommand,
    },
}

/// Result of a chain operation execution
#[derive(Debug, Clone)]
pub enum ChainOperationResult {
    /// Operation completed successfully
    Success {
        chain_id: u32,
        transaction_hash: Option<String>,
        result_data: Option<Vec<u8>>,
    },
    /// Across suggested output result
    AcrossSuggestedOutput {
        chain_id: u32,
        output_amount: Amount,
        fill_deadline: u64,
        exclusive_relayer: Address,
        exclusivity_deadline: u64,
    },
    /// Encoded calldata result
    EncodedCalldata { chain_id: u32, calldata: Vec<u8> },
    /// CA information result
    CAInfo { chain_id: u32, ca: String },
    /// Operation failed
    Failure { chain_id: u32, error: String },
}
