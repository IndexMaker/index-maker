use chrono::{DateTime, Utc};
use index_maker::core::bits::{Address, Amount, Symbol};
use index_maker::index::basket::Basket;
use alloy::primitives::FixedBytes;
use crate::custody_helper::Party;
use crate::credentials::EvmCredentials;

/// Commands that can be sent to the chain operations arbiter
#[derive(Clone)]
pub enum ChainCommand {
    /// Set solver weights for an index
    SetSolverWeights {
        symbol: Symbol,
        basket: std::sync::Arc<Basket>,
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
    /// Execute custody to connector operation
    CustodyToConnector {
        chain_id: u32,
        input_token: Address,
        amount: Amount,
        connector_address: Address,
        custody_id: FixedBytes<32>,
        party: Party,
    },
    /// Setup custody with input tokens
    SetupCustody {
        chain_id: u32,
        custody_id: FixedBytes<32>,
        input_token: Address,
        amount: Amount,
    },
    /// Approve token for spending
    ApproveToken {
        chain_id: u32,
        token_address: Address,
        spender: Address,
        amount: Amount,
    },
    /// Execute call connector operation
    CallConnector {
        chain_id: u32,
        connector_address: Address,
        calldata: Vec<u8>,
        custody_id: FixedBytes<32>,
        party: Party,
    },
    /// Get Across protocol suggested output (fees, deadlines)
    GetAcrossSuggestedOutput {
        chain_id: u32,
        input_token: Address,
        output_token: Address,
        origin_chain_id: u64,
        destination_chain_id: u64,
        amount: Amount,
    },
    /// Encode deposit calldata for Across protocol
    EncodeDepositCalldata {
        chain_id: u32,
        recipient: Address,
        input_token: Address,
        output_token: Address,
        deposit_amount: Amount,
        min_amount: Amount,
        destination_chain_id: u64,
        exclusive_relayer: Address,
        fill_deadline: u64,
        exclusivity_deadline: u64,
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
    },
    /// Get CA (Custody Account) information
    GetCA {
        chain_id: u32,
        custody_id: FixedBytes<32>,
    },
}

/// Requests for managing chain operations
/// Following the binance pattern with credential-based requests
pub enum ChainOperationRequest {
    /// Add a new chain operation using credentials (following binance pattern)
    AddOperation {
        credentials: EvmCredentials,
    },
    /// Remove a chain operation
    RemoveOperation {
        chain_id: u32,
    },
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
    EncodedCalldata {
        chain_id: u32,
        calldata: Vec<u8>,
    },
    /// CA information result
    CAInfo {
        chain_id: u32,
        ca: String,
    },
    /// Operation failed
    Failure {
        chain_id: u32,
        error: String,
    },
}