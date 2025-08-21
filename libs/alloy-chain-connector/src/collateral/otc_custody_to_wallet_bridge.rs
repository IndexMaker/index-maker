use std::sync::{Arc, RwLock};

use chrono::Utc;
use eyre::{eyre, OptionExt};
use index_core::collateral::collateral_router::{
    CollateralBridge, CollateralDesignation, CollateralRouterEvent,
};
use parking_lot::RwLock as AtomicLock;
use symm_core::core::{
    self,
    bits::{Address, Amount, Symbol},
    functional::{
        IntoObservableSingleVTable, NotificationHandlerOnce, PublishSingle, SingleObserver,
    },
};

use crate::collateral::{
    otc_custody_designation::OTCCustodyCollateralDesignation,
    wallet_designation::WalletCollateralDesignation,
};

pub struct OTCCustodyToWalletCollateralBridge {
    observer: Arc<AtomicLock<SingleObserver<CollateralRouterEvent>>>,
    custody: Arc<RwLock<OTCCustodyCollateralDesignation>>,
    wallet: Arc<RwLock<WalletCollateralDesignation>>,
}

impl OTCCustodyToWalletCollateralBridge {
    pub fn new(
        custody: Arc<RwLock<OTCCustodyCollateralDesignation>>,
        wallet: Arc<RwLock<WalletCollateralDesignation>>,
    ) -> Self {
        Self {
            observer: Arc::new(AtomicLock::new(SingleObserver::new())),
            custody,
            wallet,
        }
    }
}

impl CollateralBridge for OTCCustodyToWalletCollateralBridge {
    fn get_source(&self) -> Arc<RwLock<dyn CollateralDesignation>> {
        self.custody.clone() as Arc<RwLock<dyn CollateralDesignation>>
    }

    fn get_destination(&self) -> Arc<RwLock<dyn CollateralDesignation>> {
        self.wallet.clone() as Arc<RwLock<dyn CollateralDesignation>>
    }

    fn transfer_funds(
        &self,
        chain_id: u32,
        address: Address,
        client_order_id: core::bits::ClientOrderId,
        route_from: Symbol,
        route_to: Symbol,
        amount: Amount,
        cumulative_fee: Amount,
    ) -> eyre::Result<()> {
        let (wallet_chain_id, wallet_address, wallet_token_address, wallet_name) = {
            let wallet = self
                .wallet
                .read()
                .map_err(|err| eyre!("Failed to obtain lock on wallet: {:?}", err))?;
            (
                wallet.get_chain_id(),
                wallet.get_address(),
                wallet.get_token_address(),
                wallet.get_full_name(),
            )
        };

        (wallet_chain_id == chain_id)
            .then_some(())
            .ok_or_eyre("Incorrect chain ID")?;

        let custody = self
            .custody
            .read()
            .map_err(|err| eyre!("Failed to obtain lock on custody: {:?}", err))?;

        (wallet_chain_id == custody.get_chain_id())
            .then_some(())
            .ok_or_eyre("Incorrect chain ID")?;

        (wallet_token_address == custody.get_token_address())
            .then_some(())
            .ok_or_eyre("Incorrect token address")?;

        let custody_name = custody.get_full_name();
        let outer_observer = self.observer.clone();

        let observer = SingleObserver::new_with_fn(move |gas_amount| {
            let _ = gas_amount;
            let _ = cumulative_fee;
            outer_observer
                .read()
                .publish_single(CollateralRouterEvent::HopComplete {
                    chain_id,
                    address,
                    client_order_id: client_order_id.clone(),
                    timestamp: Utc::now(),
                    source: custody_name.clone(),
                    destination: wallet_name.clone(),
                    route_from: route_from.clone(),
                    route_to: route_to.clone(),
                    amount,
                    fee: Amount::ZERO,
                });
        });

        let error_observer = SingleObserver::new_with_fn(
            move |err| tracing::warn!(%address, "Failed to transfer funds: {:?}", err),
        );

        custody.custody_to_address(wallet_address, amount, observer, error_observer)?;

        Ok(())
    }
}

impl IntoObservableSingleVTable<CollateralRouterEvent> for OTCCustodyToWalletCollateralBridge {
    fn set_observer(&mut self, observer: Box<dyn NotificationHandlerOnce<CollateralRouterEvent>>) {
        self.observer.write().set_observer(observer);
    }
}
