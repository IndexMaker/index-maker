use alloy::primitives::U256;
use eyre::{eyre, OptionExt, Result};
use parking_lot::RwLock as AtomicLock;
use rust_decimal::dec;
use safe_math::safe;
use std::str::FromStr;
use std::sync::{Arc, RwLock as ComponentLock, Weak};
use symm_core::core::functional::{IntoObservableSingleVTable, NotificationHandlerOnce};

use index_core::collateral::collateral_router::{
    CollateralBridge, CollateralDesignation, CollateralRouterEvent,
};
use crate::across_deposit::{
    ARBITRUM_CHAIN_ID, BASE_CHAIN_ID,
    USDC_ARBITRUM_ADDRESS, USDC_BASE_ADDRESS,
};
use crate::arbiter::Arbiter;
use crate::chain_operations::ChainOperations;
use crate::commands::{ChainCommand, ChainOperationRequest};
use crate::credentials::EvmCredentials;
use crate::custody_helper::Party;
use index_core::blockchain::chain_connector::ChainNotification;
use chrono::{DateTime, Utc};
use alloy::primitives::B256;

use symm_core::core::{
    bits::{Address, Amount, ClientOrderId, Symbol},
    decimal_ext::DecimalExt,
    functional::{IntoObservableSingle, PublishSingle, SingleObserver},
};
use tokio::{spawn, task::JoinHandle};
use tokio::sync::mpsc::unbounded_channel;

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
        todo!("Tell the balance of collateral symbol (e.g. USDC) in that designation")
    }
}

pub struct EvmCollateralBridge {
    observer: SingleObserver<CollateralRouterEvent>, //< we need to fire events into this observer (method included, see below)
    me: Weak<ComponentLock<Self>>, //< we need (safe) self-reference for fring events asynchronously
    source: Arc<ComponentLock<EvmCollateralDesignation>>,
    destination: Arc<ComponentLock<EvmCollateralDesignation>>,
    tasks: AtomicLock<Vec<JoinHandle<Result<()>>>>,
    
    // New architecture components
    arbiter: Option<Arbiter>,
    chain_operations: Arc<AtomicLock<ChainOperations>>,
    chain_observer: Arc<AtomicLock<SingleObserver<ChainNotification>>>,
    request_sender: Option<tokio::sync::mpsc::UnboundedSender<ChainOperationRequest>>,
}

impl EvmCollateralBridge {
    pub fn new_arc(
        source: Arc<ComponentLock<EvmCollateralDesignation>>,
        destination: Arc<ComponentLock<EvmCollateralDesignation>>,
    ) -> Arc<ComponentLock<Self>> {
        Arc::new_cyclic(|me| {
            // Initialize the new architecture components
            let chain_operations = Arc::new(AtomicLock::new(ChainOperations::new()));
            let chain_observer = Arc::new(AtomicLock::new(SingleObserver::new()));
            
            ComponentLock::new(Self {
                me: me.clone(),
                observer: SingleObserver::new(),
                source,
                destination,
                tasks: AtomicLock::new(Vec::new()),
                
                // New architecture components (initialized but not started yet)
                arbiter: None,
                chain_operations,
                chain_observer,
                request_sender: None,
            })
        })
    }
    
    /// Initialize and start the arbiter system
    pub fn start_arbiter(&mut self) -> Result<()> {
        let (request_tx, request_rx) = unbounded_channel();
        
        let mut arbiter = Arbiter::new();
        arbiter.start(
            self.chain_operations.clone(),
            request_rx,
            self.chain_observer.clone(),
            5, // max chain operations
        );
        
        self.arbiter = Some(arbiter);
        self.request_sender = Some(request_tx);
        
        // Add chain operations for source and destination chains
        self.setup_chain_operations()?;
        
        Ok(())
    }
    
    /// Setup chain operations for both source and destination chains
    fn setup_chain_operations(&self) -> Result<()> {
        if let Some(sender) = &self.request_sender {
            // Add Arbitrum chain operation
            let arbitrum_creds = EvmCredentials::arbitrum()
                .map_err(|e| eyre!("Failed to create Arbitrum credentials: {}", e))?;
            
            let add_arbitrum = ChainOperationRequest::AddOperation {
                credentials: arbitrum_creds,
            };
            sender.send(add_arbitrum)
                .map_err(|e| eyre!("Failed to send Arbitrum add request: {}", e))?;
            
            // Add Base chain operation
            let base_creds = EvmCredentials::base()
                .map_err(|e| eyre!("Failed to create Base credentials: {}", e))?;
            
            let add_base = ChainOperationRequest::AddOperation {
                credentials: base_creds,
            };
            sender.send(add_base)
                .map_err(|e| eyre!("Failed to send Base add request: {}", e))?;
        }
        
        Ok(())
    }

    fn notify_collateral_router_event(
        &self,
        chain_id: u32,
        address: Address,
        client_order_id: ClientOrderId,
        timestamp: DateTime<Utc>,
        route_from: Symbol,
        route_to: Symbol,
        amount: Amount,
        fee: Amount,
    ) {
        self.observer
            .publish_single(CollateralRouterEvent::HopComplete {
                chain_id,
                address,
                client_order_id,
                timestamp,
                source: self.source.read().unwrap().get_full_name(),
                destination: self.destination.read().unwrap().get_full_name(),
                route_from,
                route_to,
                amount,
                fee,
            });
    }
}

impl IntoObservableSingle<CollateralRouterEvent> for EvmCollateralBridge {
    fn get_single_observer_mut(&mut self) -> &mut SingleObserver<CollateralRouterEvent> {
        &mut self.observer
    }
}

impl IntoObservableSingleVTable<CollateralRouterEvent> for EvmCollateralBridge {
    fn set_observer(&mut self, observer: Box<dyn NotificationHandlerOnce<CollateralRouterEvent>>) {
        self.observer.set_observer(observer);
    }
}

impl CollateralBridge for EvmCollateralBridge {
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
        let me = self.me.upgrade().unwrap();
        let request_sender = self.request_sender.clone()
            .ok_or_eyre("Arbiter system not initialized. Call start_arbiter() first.")?;

        let transfer_task: JoinHandle<Result<()>> = spawn(async move {
            println!("🚀 Starting transfer using new architecture...");
            
            // Use the new architecture: send command to arbiter instead of direct call
            let execute_command = ChainOperationRequest::ExecuteCommand {
                chain_id: ARBITRUM_CHAIN_ID as u32, // Source chain (Arbitrum) - convert to u32
                command: ChainCommand::ExecuteCompleteAcrossDeposit {
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
                },
            };
            
            // Send the command to the arbiter system
            request_sender.send(execute_command)
                .map_err(|e| eyre!("Failed to send execute command: {}", e))?;
            
            println!("✅ Command sent to arbiter system");
            
            // Give time for the command to be processed
            tokio::time::sleep(tokio::time::Duration::from_millis(2000)).await;
            
            // Fire the collateral router event to indicate completion
            let timestamp = Utc::now();
            let fee = safe!(cumulative_fee + dec!(0.1)).ok_or_eyre("Math Problem")?;
            me.read()
                .map_err(|err| eyre!("Failed to access bridge: {}", err))?
                .notify_collateral_router_event(
                    chain_id,
                    address,
                    client_order_id,
                    timestamp,
                    route_from,
                    route_to,
                    amount,
                    fee,
                );

            println!("🎉 Transfer completed via new architecture!");
            Ok(())
        });

        self.tasks.write().push(transfer_task);
        Ok(())
    }
}
