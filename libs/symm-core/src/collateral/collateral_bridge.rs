use std::
    sync::{Arc, RwLock as ComponentLock}
;

use chrono::{DateTime, Utc};

use eyre::Result;

use crate::core::{
    bits::{Address, Amount, ClientOrderId, Symbol},
    functional::IntoObservableSingleVTable,
};

pub enum CollateralTransferEvent {
    TransferComplete {
        chain_id: u32,
        address: Address,
        client_order_id: ClientOrderId,
        timestamp: DateTime<Utc>,
        transfer_from: Symbol,
        transfer_to: Symbol,
        amount: Amount,
        fee: Amount,
    },
}

pub enum CollateralRouterEvent {
    HopComplete {
        chain_id: u32,
        address: Address,
        client_order_id: ClientOrderId,
        timestamp: DateTime<Utc>,
        source: Symbol,
        destination: Symbol,
        route_from: Symbol,
        route_to: Symbol,
        amount: Amount,
        fee: Amount,
    },
}

pub trait CollateralDesignation: Send + Sync {
    /// e.g. EVM, CEFFU, BINANCE
    fn get_type(&self) -> Symbol;

    /// e.g. ARBITRUM, BASE, CEFFU, 1, 2
    fn get_name(&self) -> Symbol;

    /// e.g. USDC, USDT
    fn get_collateral_symbol(&self) -> Symbol;

    /// e.g. EVM:ARBITRUM:USDC
    fn get_full_name(&self) -> Symbol;

    /// Tell balance of the collateral in this designation
    fn get_balance(&self) -> Amount;
}

pub trait CollateralBridge:
    IntoObservableSingleVTable<CollateralRouterEvent> + Send + Sync
{
    /// e.g. EVM:ARBITRUM:USDC
    fn get_source(&self) -> Arc<ComponentLock<dyn CollateralDesignation>>;

    /// e.g. BINANCE:1:USDT
    fn get_destination(&self) -> Arc<ComponentLock<dyn CollateralDesignation>>;

    /// Transfer funds from this designation to target designation
    ///
    /// - chain_id - Chain ID from which IndexOrder originated
    /// - address - User address from whom IndexOrder originated
    /// - client_order_id - An ID of the IndexOrder that user assigned to it
    /// - amount - An amount to be transferred from source to destination
    ///
    /// This may be direct bridge or composite of several bridges.
    ///
    /// Bridging Rules:
    /// ===============
    /// * `EVM:ARBITRUM:USDC` => `EVM:BASE:USDC`
    /// * `EVM:BASE:USDC` => `CEFFU:USDC`
    /// * `CEFFU:USDC` => `BINANCE:1:USDC`
    /// * `BINANCE:1:USDC` => `BINANCE:1:USDT`
    /// * `BINANCE:1:USDC` => `BINANCE:2:USDC`
    fn transfer_funds(
        &self,
        chain_id: u32,
        address: Address,
        client_order_id: ClientOrderId,
        route_from: Symbol,
        route_to: Symbol,
        amount: Amount,
        cumulative_fee: Amount,
    ) -> Result<()>;
}