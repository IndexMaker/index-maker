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

use crate::chain_operations::ChainOperations;
use crate::commands::ChainCommand;
use crate::custody_helper::Party;
use crate::designation::EvmCollateralDesignation;
use crate::designation_details::EvmDesignationDetails;
use alloy::primitives::B256;
use chrono::Utc;
use index_core::collateral::collateral_router::{
    CollateralBridge, CollateralDesignation, CollateralRouterEvent,
};

use symm_core::core::{
    bits::{Address, Amount, ClientOrderId, Symbol},
    decimal_ext::DecimalExt,
    functional::{PublishSingle, SingleObserver},
};

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

        tracing::info!("Starting transfer using direct chain_operations...");

        // Get designation details
        let source_designation = self.source.read().unwrap();
        let destination_designation = self.destination.read().unwrap();
        
        // Use direct chain_operations.execute_command() instead of arbiter
        let command = ChainCommand::ExecuteCompleteAcrossDeposit {
            chain_id: source_designation.get_chain_id() as u32,
            recipient: address,
            input_token: source_designation.get_input_token_address(),
            output_token: destination_designation.get_output_token_address(),
            deposit_amount: amount,
            origin_chain_id: source_designation.get_chain_id(),
            destination_chain_id: destination_designation.get_chain_id(),
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
            operations.execute_command(source_designation.get_chain_id() as u32, command)?;
        }

        tracing::info!("Command sent to chain_operations directly");

        Ok(())
    }
}
