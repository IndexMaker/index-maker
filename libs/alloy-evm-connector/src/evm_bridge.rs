use alloy::primitives::U256;
use eyre::{eyre, OptionExt, Result};
use parking_lot::RwLock as AtomicLock;
use rust_decimal::{dec, Decimal};
use safe_math::safe;
use std::str::FromStr;
use std::sync::{Arc, RwLock as ComponentLock, Weak};

use crate::across_deposit::{
    new_builder_from_env, AcrossDepositBuilder, ARBITRUM_CHAIN_ID, BASE_CHAIN_ID,
    USDC_ARBITRUM_ADDRESS, USDC_BASE_ADDRESS,
};
use chrono::{DateTime, Utc};
use index_maker::{
    collateral::collateral_router::{
        CollateralBridge, CollateralDesignation, CollateralRouterEvent,
    },
    core::{
        bits::{Address, Amount, ClientOrderId, Symbol},
        decimal_ext::DecimalExt,
        functional::{IntoObservableSingle, PublishSingle, SingleObserver},
    },
};
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
        todo!("Tell the balance of collateral symbol (e.g. USDC) in that designation")
    }
}

pub struct EvmCollateralBridge {
    observer: SingleObserver<CollateralRouterEvent>, //< we need to fire events into this observer (method included, see below)
    me: Weak<ComponentLock<Self>>, //< we need (safe) self-reference for fring events asynchronously
    source: Arc<ComponentLock<EvmCollateralDesignation>>,
    destination: Arc<ComponentLock<EvmCollateralDesignation>>,
    tasks: AtomicLock<Vec<JoinHandle<Result<()>>>>,
}

impl EvmCollateralBridge {
    pub fn new_arc(
        source: Arc<ComponentLock<EvmCollateralDesignation>>,
        destination: Arc<ComponentLock<EvmCollateralDesignation>>,
    ) -> Arc<ComponentLock<Self>> {
        Arc::new_cyclic(|me| {
            ComponentLock::new(Self {
                me: me.clone(),
                observer: SingleObserver::new(),
                source,
                destination,
                tasks: AtomicLock::new(Vec::new()),
            })
        })
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

        let transfer_task: JoinHandle<Result<()>> = spawn(async move {
            // NOTE: I'm putting this example piece inside tokio::spawn() as I
            // am aware that Alloy might need async code. Use this piece to
            // start tokio async task, and once you confirm transfer to
            // designation, please fire the event as shown below.
            //
            // Fill in... Need to implement transfering funds
            // - from: self.source
            // - to: self.destination
            //
            // The `route_from` and `route_to`` parameters should be ignored and
            // passed through to the collateral routing event.  These two
            // parameters are used to tell router which path this event is for,
            // but our transfer_funds() is to transfer only between self.source
            // to self.destination, and only in that one direction.
            //
            // The below code shows how to fire the event, and it should be used
            // when transfer is actually complete. Collateral router responds to
            // those events by producing next hop, which invokes next bridge (a
            // different one than this)
            //
            let deposit_builder = new_builder_from_env().await.unwrap();
            deposit_builder
                .execute_complete_across_deposit(
                    address,
                    USDC_ARBITRUM_ADDRESS,
                    USDC_BASE_ADDRESS,
                    U256::from_str(
                        &(amount.try_into().unwrap_or(0u128) * 1_000_000u128).to_string(),
                    )
                    .unwrap(), // 6 decimals for USDC
                    ARBITRUM_CHAIN_ID,
                    BASE_CHAIN_ID,
                )
                .await
                .unwrap();

            //
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

            Ok(())
        });

        self.tasks.write().push(transfer_task);
        Ok(())
    }
}
