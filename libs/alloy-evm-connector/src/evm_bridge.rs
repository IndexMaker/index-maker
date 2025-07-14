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

pub struct EvmCollateralBridge {
    observer: Arc<AtomicLock<SingleObserver<CollateralRouterEvent>>>,
    source: Arc<ComponentLock<EvmCollateralDesignation>>,
    destination: Arc<ComponentLock<EvmCollateralDesignation>>,

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
        Arc::new({
            // Initialize the new architecture components
            let chain_operations = Arc::new(AtomicLock::new(ChainOperations::new()));
            let chain_observer = Arc::new(AtomicLock::new(SingleObserver::new()));

            ComponentLock::new(Self {
                observer: Arc::new(AtomicLock::new(SingleObserver::new())),
                source,
                destination,

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

        // You will have one Chain Connector and multiple Bridges.
        // Each bridge is for exact source -> destination routing, so
        // for each (source -> destination) pair there will be separate bridge created.
        todo!("Chain Connector should be the one owning the arbiter");

        Ok(())
    }

    /// Setup chain operations for both source and destination chains.
    ///
    /// Sadhbh: Move all that into EvmConnector.
    ///
    /// There is only one EvmConnector and there are multiple EvmBridges.
    ///
    /// The way I want to setup application is:
    /// - Create EvmConnector, which should create Arbiter
    /// - Create EvmCollateralDesignation for each custory on each network for each currency
    /// - Create EvmCollateralBridge for each (source -> destination) pair
    ///
    /// EvmConnector should own Arbiter and start it.
    /// The EvmConnector should also provide observer for solver chain events (solver weights, deposit, withdrawal request).
    ///
    /// - EvmCollateralDesignation(s) should be derived from EvmConnector
    /// - EvmCollateralBridge(s) should be derived from EvmConnector and
    ///   related to source and destination EvmCollateralDesignation(s).
    ///
    /// The source and destination are telling what is the bridge for,
    /// and perhaps the process of creating EvmCollateralDesignation should
    /// be the one performing ChainOperationRequest::AddOperation command.
    ///
    /// EvmCollateralBridge should be small and it should merely provide:
    /// - An observer for CollateralRouterEvent
    /// - A transfer_funds() function that will enqueue a command into Arbiter
    ///
    /// Note: An observer can be put into Arc<AtomicLock<..>> (already shown above).
    /// We are calling observer.write() to call set_observer(fn), and we are calling
    /// obserer.read() to publish. The publish is always non-blocking and thus safe
    /// to call from async context. When we pass events into channels we use
    /// UnboundedSender to ensure operation is non-blicking. Code should not contain
    /// any calls to tokio::spawn(). All asynchronous work should be done in AsyncLoop(s)
    /// of Arbiter and related ChainOperation, and non-async context should be notified
    /// via callbacks (as shown). I use here Arc<dyn Fn()> as callback, that is because
    /// ChainCommand implements Clone. A good question to ask is why Clone is needed?
    /// ChainCommand is something we pass from source context to execution context, so
    /// ownership of the command moves at all times. There is no need to copy commands.
    /// Thus if Clone can be removed, the callback can be passed as Box<FnOnce()>, which
    /// is much better option as it allows callback closure data to be moved out of callback
    /// once it's executed.
    ///
    fn setup_chain_operations(&self) -> Result<()> {
        if let Some(sender) = &self.request_sender {
            // Add Arbitrum chain operation
            let arbitrum_creds = EvmCredentials::arbitrum()
                .map_err(|e| eyre!("Failed to create Arbitrum credentials: {}", e))?;

            let add_arbitrum = ChainOperationRequest::AddOperation {
                credentials: arbitrum_creds,
            };
            sender
                .send(add_arbitrum)
                .map_err(|e| eyre!("Failed to send Arbitrum add request: {}", e))?;

            // Add Base chain operation
            let base_creds = EvmCredentials::base()
                .map_err(|e| eyre!("Failed to create Base credentials: {}", e))?;

            let add_base = ChainOperationRequest::AddOperation {
                credentials: base_creds,
            };
            sender
                .send(add_base)
                .map_err(|e| eyre!("Failed to send Base add request: {}", e))?;
        }

        Ok(())
    }
}

impl IntoObservableSingleArc<CollateralRouterEvent> for EvmCollateralBridge {
    fn get_single_observer_arc(
        &mut self,
    ) -> &Arc<AtomicLock<SingleObserver<CollateralRouterEvent>>> {
        &self.observer
    }
}

impl IntoObservableSingleVTable<CollateralRouterEvent> for EvmCollateralBridge {
    fn set_observer(&mut self, observer: Box<dyn NotificationHandlerOnce<CollateralRouterEvent>>) {
        self.observer.write().set_observer(observer);
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
        let observer = self.observer.clone();
        let source = self.source.read().unwrap().get_full_name();
        let destination = self.destination.read().unwrap().get_full_name();
        let request_sender = self
            .request_sender
            .clone()
            .ok_or_eyre("Arbiter system not initialized. Call start_arbiter() first.")?;

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
            },
        };

        // Send the command to the arbiter system
        request_sender
            .send(execute_command)
            .map_err(|e| eyre!("Failed to send execute command: {}", e))?;

        println!("✅ Command sent to arbiter system");

        Ok(())
    }
}
