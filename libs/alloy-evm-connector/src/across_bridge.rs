use alloy::primitives::U256;
use eyre::{eyre, OptionExt, Result};
use parking_lot::RwLock as AtomicLock;
use rust_decimal::dec;
use safe_math::safe;
use std::str::FromStr;
use std::sync::{Arc, RwLock as ComponentLock, Weak};
use symm_core::core::functional::{
    IntoObservableSingleArc, IntoObservableSingleVTable, NotificationHandlerOnce,
};

use crate::across_deposit::{
    ARBITRUM_CHAIN_ID, BASE_CHAIN_ID, USDC_ARBITRUM_ADDRESS, USDC_BASE_ADDRESS,
};
use crate::arbiter::Arbiter;
use crate::chain_operations::ChainOperations;
use crate::commands::{ChainCommand, ChainOperationRequest};
use crate::credentials::EvmCredentials;
use crate::custody_helper::Party;
use alloy::primitives::B256;
use chrono::{DateTime, Utc};
use index_core::blockchain::chain_connector::ChainNotification;
use index_core::collateral::collateral_router::{
    CollateralBridge, CollateralDesignation, CollateralRouterEvent,
};

use symm_core::core::{
    bits::{Address, Amount, ClientOrderId, Symbol},
    decimal_ext::DecimalExt,
    functional::{IntoObservableSingle, PublishSingle, SingleObserver},
};
use tokio::sync::mpsc::unbounded_channel;
use tokio::{spawn, task::JoinHandle};

const BRIDGE_TYPE: &str = "EVM";

pub struct EvmCollateralDesignation {
    pub name: Symbol,              //< e.g. "ARBITRUM", or "BASE"
    pub collateral_symbol: Symbol, //< should be "USDC" (in future could also be "USDT")
    pub full_name: Symbol,         //< e.g. "EVM:ARBITRUM:USDC"
}

impl CollateralDesignation for EvmCollateralDesignation {
    fn get_type(&self) -> Symbol {
        BRIDGE_TYPE.into()
    }
    fn get_name(&self) -> Symbol {
        self.name.clone()
    }
    fn get_collateral_symbol(&self) -> Symbol {
        self.collateral_symbol.clone()
    }
    fn get_full_name(&self) -> Symbol {
        self.full_name.clone() //< for preformance better to pre-construct than format on-demand
    }
    fn get_balance(&self) -> Amount {
        // TODO: Should enqueue ChainCommand to retrieve balance at given designation
        // i.e. balance of our custody on network X in currency Y
        todo!("Tell the balance of collateral symbol (e.g. USDC) in that designation")
    }
}

pub struct AcrossCollateralBridge {
    observer: Arc<AtomicLock<SingleObserver<CollateralRouterEvent>>>,
    source: Arc<ComponentLock<EvmCollateralDesignation>>,
    destination: Arc<ComponentLock<EvmCollateralDesignation>>,

    // Shared chain_operations injected by EvmConnector
    chain_operations: Arc<AtomicLock<ChainOperations>>,
}

impl AcrossCollateralBridge {
    /// Constructor that accepts shared chain_operations from EvmConnector
    pub fn new_with_shared_operations(
        source: Arc<ComponentLock<EvmCollateralDesignation>>,
        destination: Arc<ComponentLock<EvmCollateralDesignation>>,
        chain_operations: Arc<AtomicLock<ChainOperations>>,
    ) -> Arc<ComponentLock<Self>> {
        Arc::new({
            ComponentLock::new(Self {
                observer: Arc::new(AtomicLock::new(SingleObserver::new())),
                source,
                destination,
                chain_operations,
            })
        })
    }
}

impl IntoObservableSingleVTable<CollateralRouterEvent> for AcrossCollateralBridge {
    fn set_observer(&mut self, observer: Box<dyn NotificationHandlerOnce<CollateralRouterEvent>>) {
        self.observer.write().set_observer(observer);
    }
}

impl CollateralBridge for AcrossCollateralBridge {
    fn get_source(&self) -> Arc<ComponentLock<dyn CollateralDesignation>> {
        (self.source).clone() as Arc<ComponentLock<dyn CollateralDesignation>>
    }

    fn get_destination(&self) -> Arc<ComponentLock<dyn CollateralDesignation>> {
        (self.destination).clone() as Arc<ComponentLock<dyn CollateralDesignation>>
    }

    fn transfer_funds(
        &self,
        chain_id: u32,
        address: Address,
        client_order_id: ClientOrderId,
        route_from: Symbol,
        route_to: Symbol,
        amount: Amount,
        cumulative_fee: Amount,
    ) -> Result<()> {
        let observer = self.observer.clone();
        let source = self.source.read().unwrap().get_full_name();
        let destination = self.destination.read().unwrap().get_full_name();

        println!("🚀 Starting transfer using direct chain_operations...");

        // Use direct chain_operations.execute_command() instead of arbiter
        let command = ChainCommand::ExecuteCompleteAcrossDeposit {
            chain_id: ARBITRUM_CHAIN_ID as u32,
            recipient: address,
            input_token: USDC_ARBITRUM_ADDRESS,
            output_token: USDC_BASE_ADDRESS,
            deposit_amount: amount,
            origin_chain_id: ARBITRUM_CHAIN_ID,
            destination_chain_id: BASE_CHAIN_ID,
            party: Party {
                parity: 0,
                x: B256::ZERO,
            },
            callback: Arc::new(move |total_routed, fee_deducted| {
                let timestamp = Utc::now();
                let fee = safe!(cumulative_fee + fee_deducted).ok_or_eyre("Math problem")?;
                observer
                    .read()
                    .publish_single(CollateralRouterEvent::HopComplete {
                        chain_id,
                        address,
                        client_order_id: client_order_id.clone(),
                        timestamp,
                        source: source.clone(),
                        destination: destination.clone(),
                        route_from: route_from.clone(),
                        route_to: route_to.clone(),
                        amount: total_routed,
                        fee,
                    });
                Ok(())
            }),
        };

        // Send the command directly to chain_operations
        {
            let operations = self.chain_operations.read();
            operations.execute_command(ARBITRUM_CHAIN_ID as u32, command)?;
        }

        println!("✅ Command sent to chain_operations directly");

        Ok(())
    }
}
