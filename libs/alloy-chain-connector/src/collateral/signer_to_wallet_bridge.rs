use std::sync::{Arc, RwLock};

use chrono::Utc;
use eyre::{eyre, OptionExt};
use index_core::collateral::collateral_router::{
    CollateralBridge, CollateralDesignation, CollateralRouterEvent, CollateralRoutingStatus,
};
use parking_lot::RwLock as AtomicLock;
use rust_decimal::dec;
use safe_math::safe;
use symm_core::core::{
    self,
    bits::{Address, Amount, Symbol},
    decimal_ext::DecimalExt,
    functional::{
        IntoObservableSingleVTable, IntoObservableSingleVTableRef, NotificationHandlerOnce, OneShotSingleObserver, PublishSingle, SingleObserver
    },
};

use crate::{
    chain_connector::GasFeeCalculator,
    collateral::{
        signer_wallet_designation::SignerWalletCollateralDesignation,
        wallet_designation::WalletCollateralDesignation,
    },
};

pub struct SignerWalletToWalletCollateralBridge {
    observer: Arc<AtomicLock<SingleObserver<CollateralRouterEvent>>>,
    signer_wallet: Arc<SignerWalletCollateralDesignation>,
    wallet: Arc<WalletCollateralDesignation>,
    gas_fee_calculator: GasFeeCalculator,
}

impl SignerWalletToWalletCollateralBridge {
    pub fn new(
        custody: Arc<SignerWalletCollateralDesignation>,
        wallet: Arc<WalletCollateralDesignation>,
        gas_fee_calculator: GasFeeCalculator,
    ) -> Self {
        Self {
            observer: Arc::new(AtomicLock::new(SingleObserver::new())),
            signer_wallet: custody,
            wallet,
            gas_fee_calculator,
        }
    }
}

impl CollateralBridge for SignerWalletToWalletCollateralBridge {
    fn get_source(&self) -> Arc<dyn CollateralDesignation> {
        self.signer_wallet.clone() as Arc<dyn CollateralDesignation>
    }

    fn get_destination(&self) -> Arc<dyn CollateralDesignation> {
        self.wallet.clone() as Arc<dyn CollateralDesignation>
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
        let (wallet_chain_id, wallet_address, wallet_token_address, wallet_name) = (
            self.wallet.get_chain_id(),
            self.wallet.get_address(),
            self.wallet.get_token_address(),
            self.wallet.get_full_name(),
        );

        (wallet_chain_id == chain_id)
            .then_some(())
            .ok_or_eyre("Incorrect chain ID")?;

        (wallet_chain_id == self.signer_wallet.get_chain_id())
            .then_some(())
            .ok_or_eyre("Incorrect chain ID")?;

        (wallet_token_address == self.signer_wallet.get_token_address())
            .then_some(())
            .ok_or_eyre("Incorrect token address")?;

        let signer_wallet_name = self.signer_wallet.get_full_name();
        let outer_observer = self.observer.clone();
        let outer_observer_clone = self.observer.clone();
        let gas_fee_calculator = self.gas_fee_calculator.clone();

        let client_order_id_clone = client_order_id.clone();
        let client_order_id_clone_2 = client_order_id.clone();
        let source_clone = signer_wallet_name.clone();
        let destination_clone = wallet_name.clone();
        let route_from_clone = route_from.clone();
        let route_to_clone = route_to.clone();

        // Charge at most 10%, we'll take the hit
        // TODO: Configure me
        let max_fee_rate = dec!(0.1);
        let max_fee = safe!(amount * max_fee_rate).ok_or_eyre("Math problem")?;

        let compute_fee = move |gas_amount_eth| -> eyre::Result<(Amount, Amount)> {
            let gas_fee = gas_fee_calculator.compute_amount(gas_amount_eth)?;
            let chargeable_fee = gas_fee.min(max_fee);
            tracing::info!(
                %chain_id, %address, %client_order_id, %chargeable_fee, %gas_fee,
                "Computing gas fee"
            );
            let cumulative_fee =
                safe!(cumulative_fee + chargeable_fee).ok_or_eyre("Math problem")?;
            let amount = safe!(amount - chargeable_fee).ok_or_eyre("Math problem")?;
            Ok((amount, cumulative_fee))
        };

        let observer = OneShotSingleObserver::new_with_fn(move |gas_amount_eth| {
            let (amount, cumulative_fee) = match compute_fee(gas_amount_eth) {
                Ok((amount, cumulative_fee)) => {
                    tracing::info!(
                        "✅ Collateral routed successfully: {}, cumulative fee: {}",
                        amount,
                        cumulative_fee
                    );
                    (amount, cumulative_fee)
                }
                Err(err) => {
                    tracing::warn!("❗️ Failed to compute collateral routing fee: {:?}", err);
                    (amount, Amount::ZERO)
                }
            };
            outer_observer
                .read()
                .publish_single(CollateralRouterEvent::HopComplete {
                    chain_id,
                    address,
                    client_order_id: client_order_id_clone,
                    timestamp: Utc::now(),
                    source: signer_wallet_name,
                    destination: wallet_name,
                    route_from,
                    route_to,
                    amount,
                    fee: cumulative_fee,
                    status: CollateralRoutingStatus::Success,
                });
        });

        let error_observer = OneShotSingleObserver::new_with_fn(move |err| {
            tracing::warn!(%address, "Failed to transfer funds: {:?}", err);
            outer_observer_clone
                .read()
                .publish_single(CollateralRouterEvent::HopComplete {
                    chain_id,
                    address,
                    client_order_id: client_order_id_clone_2,
                    timestamp: Utc::now(),
                    source: source_clone,
                    destination: destination_clone,
                    route_from: route_from_clone,
                    route_to: route_to_clone,
                    amount,
                    fee: cumulative_fee,
                    status: CollateralRoutingStatus::Failure {
                        reason: format!("Failed to transfer from wallet to wallet: {:?}", err),
                    },
                });
        });

        self.signer_wallet.transfer_to_account(wallet_address, amount, observer, error_observer)?;

        Ok(())
    }
}

impl IntoObservableSingleVTableRef<CollateralRouterEvent> for SignerWalletToWalletCollateralBridge {
    fn set_observer(&self, observer: Box<dyn NotificationHandlerOnce<CollateralRouterEvent>>) {
        self.observer.write().set_observer(observer);
    }
}
