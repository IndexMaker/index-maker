use std::{
    collections::{HashMap, VecDeque},
    sync::{Arc, RwLock as ComponentLock},
};

use chrono::{DateTime, Utc};
use itertools::{Either, Itertools};
use parking_lot::RwLock;

use eyre::{eyre, OptionExt, Result};

use derive_with_baggage::WithBaggage;
use opentelemetry::propagation::Injector;
use symm_core::core::telemetry::{TracingData, WithBaggage, WithTracingContext};

use symm_core::core::{
    bits::{Address, Amount, ClientOrderId, PaymentId, Side},
    functional::{IntoObservableSingle, PublishSingle, SingleObserver},
};
use tracing::{span, Level};

use crate::solver::solver::{CollateralManagement, SetSolverOrderStatus};

use index_core::collateral::collateral_router::{CollateralRouter, CollateralTransferEvent};

use super::collateral_position::*;

#[derive(WithBaggage)]
pub enum CollateralEvent {
    CollateralReady {
        #[baggage]
        chain_id: u32,

        #[baggage]
        address: Address,

        #[baggage]
        client_order_id: ClientOrderId,

        collateral_amount: Amount,
        timestamp: DateTime<Utc>,
        status: RoutingStatus,
    },
    PreAuthResponse {
        #[baggage]
        chain_id: u32,

        #[baggage]
        address: Address,

        #[baggage]
        client_order_id: ClientOrderId,

        amount_payable: Amount,
        timestamp: DateTime<Utc>,
        status: PreAuthStatus,
    },
    ConfirmResponse {
        #[baggage]
        chain_id: u32,

        #[baggage]
        address: Address,

        #[baggage]
        client_order_id: ClientOrderId,

        #[baggage]
        payment_id: PaymentId,

        amount_paid: Amount,
        timestamp: DateTime<Utc>,
        status: ConfirmStatus,
        position: CollateralPosition,
    },
}

pub trait CollateralManagerHost: SetSolverOrderStatus {
    fn get_next_payment_id(&self) -> PaymentId;
}

pub struct CollateralManager {
    observer: SingleObserver<CollateralEvent>,
    router: Arc<ComponentLock<CollateralRouter>>,
    client_funds: HashMap<(u32, Address), Arc<RwLock<CollateralPosition>>>,
    collateral_management_requests: VecDeque<CollateralManagement>,
    zero_threshold: Amount,
}

impl CollateralManager {
    pub fn new(router: Arc<ComponentLock<CollateralRouter>>, zero_threshold: Amount) -> Self {
        Self {
            observer: SingleObserver::new(),
            router,
            client_funds: HashMap::new(),
            collateral_management_requests: VecDeque::new(),
            zero_threshold,
        }
    }

    pub fn process_collateral(
        &mut self,
        _host: &dyn CollateralManagerHost,
        timestamp: DateTime<Utc>,
    ) -> Result<()> {
        let process_collateral_span = span!(Level::TRACE, "process-collateral");
        let _guard = process_collateral_span.enter();

        let requests = VecDeque::from_iter(self.collateral_management_requests.drain(..));

        let (ready_to_route, check_later): (Vec<_>, Vec<_>) =
            requests.into_iter().partition_map(|request| {
                if let Some(position) = self.get_position(request.chain_id, &request.address) {
                    let position_read = position.read();
                    let unconfirmed_balance = match request.side {
                        Side::Buy => position_read.side_cr.unconfirmed_balance,
                        Side::Sell => position_read.side_dr.unconfirmed_balance,
                    };
                    if unconfirmed_balance < request.collateral_amount {
                        tracing::debug!(
                            chain_id = %request.chain_id,
                            address = %request.address,
                            client_order_id = %request.client_order_id,
                            collateral_amount = %request.collateral_amount,
                            %unconfirmed_balance,
                            "Awaiting more deposit",
                        );
                        Either::Right(request)
                    } else {
                        tracing::info!(
                            chain_id = %request.chain_id,
                            address = %request.address,
                            client_order_id = %request.client_order_id,
                            collateral_amount = %request.collateral_amount,
                            %unconfirmed_balance,
                            "Ready to route",
                        );
                        Either::Left(request)
                    }
                } else {
                    tracing::debug!(
                        chain_id = %request.chain_id,
                        address = %request.address,
                        client_order_id = %request.client_order_id,
                        collateral_amount = %request.collateral_amount,
                        "Awaiting deposit",
                    );
                    Either::Right(request)
                }
            });

        self.collateral_management_requests.extend(check_later);

        let failures = ready_to_route
            .into_iter()
            .filter_map(|request| {
                request.add_span_context_link();
                match self
                    .router
                    .write()
                    .map_err(|e| eyre!("Failed to access router {}", e))
                    .and_then(|x| {
                        x.transfer_collateral(
                            request.chain_id,
                            request.address,
                            request.client_order_id.clone(),
                            request.side,
                            request.collateral_amount,
                        )
                    }) {
                    Ok(()) => None,
                    Err(err) => Some((err, request)),
                }
            })
            .collect_vec();

        if !failures.is_empty() {
            tracing::warn!(
                "Errors in processing: {}",
                failures
                    .iter()
                    .map(|(err, request)| {
                        format!(
                            "\n in {} {} {}: {:?}",
                            request.chain_id,
                            request.address,
                            request.client_order_id.clone(),
                            err
                        )
                    })
                    .join(", ")
            );
            for (_, request) in failures {
                self.observer
                    .publish_single(CollateralEvent::CollateralReady {
                        chain_id: request.chain_id,
                        address: request.address,
                        client_order_id: request.client_order_id,
                        timestamp,
                        collateral_amount: request.collateral_amount,
                        status: RoutingStatus::NotReady,
                    });
            }
        }

        Ok(())
    }

    pub fn manage_collateral(&mut self, collateral_management: CollateralManagement) {
        tracing::info!(
            chain_id = %collateral_management.chain_id,
            address = %collateral_management.address,
            client_order_id = %collateral_management.client_order_id,
            "Manage Collateral",
        );
        self.collateral_management_requests
            .push_back(collateral_management);
    }

    pub fn handle_deposit(
        &mut self,
        host: &dyn CollateralManagerHost,
        chain_id: u32,
        address: Address,
        amount: Amount,
        timestamp: DateTime<Utc>,
    ) -> Result<()> {
        tracing::info!(%chain_id, %address, %amount, "Deposit");

        let payment_id = host.get_next_payment_id();
        self.add_position(chain_id, address, timestamp)
            .write()
            .deposit(payment_id, amount, timestamp)
    }

    pub fn handle_withdrawal(
        &mut self,
        host: &dyn CollateralManagerHost,
        chain_id: u32,
        address: Address,
        amount: Amount,
        timestamp: DateTime<Utc>,
    ) -> Result<()> {
        tracing::info!(%chain_id, %address, %amount, "Withdrawal");

        let payment_id = host.get_next_payment_id();
        self.add_position(chain_id, address, timestamp)
            .write()
            .withdraw(payment_id, amount, timestamp)
    }

    /// Pre-Authorize Payment
    ///
    /// Note: For bigger Index Orders we will pre-authorize certain amount
    /// payable before we start processing Index Order. For smaller Index Orders
    /// we may have some margin to process them before payment is authorized.
    pub fn preauth_payment(
        &self,
        host: &dyn CollateralManagerHost,
        chain_id: u32,
        address: &Address,
        client_order_id: &ClientOrderId,
        timestamp: DateTime<Utc>,
        side: Side,
        amount_payable: Amount,
    ) -> Result<()> {
        tracing::info!(%chain_id, %address, %amount_payable, "PreAuth Payment");

        let funds = self
            .get_position(chain_id, &address)
            .ok_or_eyre("Failed to find position")?;

        let status = funds
            .write()
            .preauth_payment(
                &client_order_id,
                timestamp,
                side,
                amount_payable,
                self.zero_threshold,
                || host.get_next_payment_id(),
            )
            .ok_or_eyre("Math Problem")?;

        self.observer
            .publish_single(CollateralEvent::PreAuthResponse {
                chain_id,
                address: *address,
                client_order_id: client_order_id.clone(),
                amount_payable,
                timestamp,
                status,
            });

        Ok(())
    }

    /// Confirm Payment
    ///
    /// Note: Once Index Order is fully-filled, we will calculate all the costs
    /// associated and we will charge user account that amount. Index Order is
    /// processed for as long as there is some collateral left, so that user
    /// will get the maxim amount of index token for the collateral they
    /// provided. The quantity that user receives depends on market dynamics.
    pub fn confirm_payment(
        &mut self,
        chain_id: u32,
        address: &Address,
        client_order_id: &ClientOrderId,
        payment_id: &PaymentId,
        timestamp: DateTime<Utc>,
        side: Side,
        amount_paid: Amount,
    ) -> Result<()> {
        tracing::info!(%chain_id, %address, %amount_paid, "Confirm Payment");

        let funds = self
            .get_position(chain_id, &address)
            .ok_or_eyre("Failed to find position")?;

        let (status, position) = (|funds_write: &mut CollateralPosition| 
            -> Result<(ConfirmStatus, CollateralPosition)>{
            Ok((funds_write
                .confirm_payment(
                    payment_id,
                    timestamp,
                    side,
                    amount_paid,
                    self.zero_threshold,
                ).ok_or_eyre("Math Problem")?, 
                funds_write.clone()))
        })(&mut funds.write())?;

        self.observer
            .publish_single(CollateralEvent::ConfirmResponse {
                chain_id,
                address: *address,
                client_order_id: client_order_id.clone(),
                payment_id: payment_id.clone(),
                amount_paid,
                timestamp,
                status,
                position,
            });

        Ok(())
    }

    fn add_position(
        &mut self,
        chain_id: u32,
        address: Address,
        timestamp: DateTime<Utc>,
    ) -> Arc<RwLock<CollateralPosition>> {
        self.client_funds
            .entry((chain_id, address))
            .or_insert_with(|| {
                Arc::new(RwLock::new(CollateralPosition::new(
                    chain_id, address, timestamp,
                )))
            })
            .clone()
    }

    fn get_position(
        &self,
        chain_id: u32,
        address: &Address,
    ) -> Option<&Arc<RwLock<CollateralPosition>>> {
        self.client_funds.get(&(chain_id, address.clone()))
    }

    pub fn handle_collateral_transfer_event(
        &mut self,
        event: CollateralTransferEvent,
    ) -> Result<()> {
        match event {
            CollateralTransferEvent::TransferComplete {
                chain_id,
                address,
                client_order_id,
                timestamp,
                transfer_from,
                transfer_to,
                amount,
                fee,
            } => {
                tracing::info!(%chain_id, %address, %client_order_id,
                    %transfer_from, %transfer_to, %amount, %fee,
                    "Transfer Complete"
                );
                let funds = self
                    .get_position(chain_id, &address)
                    .ok_or_eyre("Failed to find position")?;

                let mut funds_write = funds.write();

                // TODO: We need to also support Sell & Withdraw
                let side = Side::Buy;

                funds_write
                    .add_ready(side, amount, timestamp, self.zero_threshold)
                    .ok_or_eyre("Math Problem")?;

                // Note: We must charge the collateral routing fee otherwise
                // we'll have dangling unconfirmed amount, while that amount was
                // in fact used already when routing collateral.
                let get_payment_id = || "Charges".into();
                let payment_id = get_payment_id();

                funds_write
                    .add_ready(side, fee, timestamp, self.zero_threshold)
                    .ok_or_eyre("Math Problem")?;

                funds_write
                    .preauth_payment(
                        &client_order_id,
                        timestamp,
                        side,
                        fee,
                        self.zero_threshold,
                        get_payment_id,
                    )
                    .ok_or_eyre("Math Problem")?;

                funds_write
                    .confirm_payment(&payment_id, timestamp, side, fee, self.zero_threshold)
                    .ok_or_eyre("Math Problem")?;

                self.observer
                    .publish_single(CollateralEvent::CollateralReady {
                        chain_id,
                        address,
                        client_order_id,
                        timestamp,
                        collateral_amount: amount,
                        status: RoutingStatus::Ready { fee },
                    });
            }
        }
        Ok(())
    }
}

impl IntoObservableSingle<CollateralEvent> for CollateralManager {
    fn get_single_observer_mut(&mut self) -> &mut SingleObserver<CollateralEvent> {
        &mut self.observer
    }
}

#[cfg(test)]
mod test {
    use std::{
        collections::HashMap,
        sync::{
            atomic::{AtomicUsize, Ordering},
            Arc, RwLock as ComponentLock,
        },
    };

    use chrono::Utc;
    use rust_decimal::dec;

    use symm_core::{
        assert_decimal_approx_eq,
        core::{
            bits::{PaymentId, Side},
            functional::IntoObservableSingle,
            telemetry::TracingData,
            test_util::{
                flag_mock_atomic_bool, get_mock_address_1, get_mock_asset_name_1,
                get_mock_asset_name_2, get_mock_atomic_bool_pair, get_mock_defer_channel,
                run_mock_deferred, test_mock_atomic_bool,
            },
        },
    };

    use crate::{
        collateral::{collateral_manager::PreAuthStatus, collateral_position::RoutingStatus},
        solver::{
            solver::{CollateralManagement, SetSolverOrderStatus},
            solver_order::{SolverOrder, SolverOrderStatus},
            solver_quote::{SolverQuote, SolverQuoteStatus},
        },
    };
    use index_core::collateral::collateral_router::test_util::build_test_router;

    use super::{CollateralEvent, CollateralManager, CollateralManagerHost, ConfirmStatus};

    struct MockCollateralManagerHost {
        last_id: AtomicUsize,
    }

    impl MockCollateralManagerHost {
        pub fn new() -> Self {
            Self {
                last_id: AtomicUsize::new(0),
            }
        }
    }

    impl SetSolverOrderStatus for MockCollateralManagerHost {
        fn set_order_status(&self, order: &mut SolverOrder, status: SolverOrderStatus) {
            order.status = status;
        }

        fn set_quote_status(&self, order: &mut SolverQuote, status: SolverQuoteStatus) {
            todo!()
        }
    }
    impl CollateralManagerHost for MockCollateralManagerHost {
        fn get_next_payment_id(&self) -> PaymentId {
            let last_id = self.last_id.fetch_add(1, Ordering::SeqCst);
            PaymentId::from(format!("P-{}", last_id))
        }
    }

    /// Test Collateral Manager
    /// ------
    /// Collateral Manager maintains accounts of user's collateral, and
    /// delegates routing to Collateral Router.
    ///
    /// This test uses mocked collateral router with two routes possible, and
    /// then we follow typical lifecycle of the collateral from deposit, through
    /// routing, to spending on minting.
    ///
    #[test]
    fn test_collateral_manager() {
        let zero_threshold = dec!(0.0001);
        let timestamp = Utc::now();

        let host = MockCollateralManagerHost::new();

        let (tx, rx) = get_mock_defer_channel();

        let router = build_test_router(
            &tx,
            &["T1:N1:C1", "T2:N2:C2", "T3:N3:C3", "T4:N4:C4"],
            &[
                ("T1:N1:C1", "T3:N3:C3"),
                ("T2:N2:C2", "T3:N3:C3"),
                ("T3:N3:C3", "T4:N4:C4"),
            ],
            &[
                &["T1:N1:C1", "T3:N3:C3", "T4:N4:C4"],
                &["T2:N2:C2", "T3:N3:C3", "T4:N4:C4"],
            ],
            &[(1, "T1:N1:C1"), (2, "T2:N2:C2")],
            "T4:N4:C4",
            |amount, cumulative_fee| {
                let fee = dec!(0.5);
                let cumulative_fee = cumulative_fee + fee;
                (amount - fee, cumulative_fee)
            },
        );

        let collateral_manager = Arc::new(ComponentLock::new(CollateralManager::new(
            router.clone(),
            zero_threshold,
        )));

        let collateral_manager_weak = Arc::downgrade(&collateral_manager);
        router
            .write()
            .unwrap()
            .get_single_observer_mut()
            .set_observer_fn(move |e| {
                let collateral_manager = collateral_manager_weak.upgrade().unwrap();
                tx.send(Box::new(move || {
                    collateral_manager
                        .write()
                        .unwrap()
                        .handle_collateral_transfer_event(e)
                        .unwrap();
                }))
                .unwrap();
            });

        let (preauth_approved_set, preauth_approved_get) = get_mock_atomic_bool_pair();
        let (confirm_auth_set, confirm_auth_get) = get_mock_atomic_bool_pair();

        collateral_manager
            .write()
            .unwrap()
            .get_single_observer_mut()
            .set_observer_fn(move |e| match e {
                CollateralEvent::CollateralReady {
                    collateral_amount,
                    status,
                    ..
                } => match status {
                    RoutingStatus::Ready { fee } => {
                        tracing::info!(
                            "Collateral Ready Event {:0.5} {:0.5}",
                            collateral_amount,
                            fee
                        );
                    }
                    RoutingStatus::NotReady => {
                        tracing::warn!(
                            "Collateral Ready Event {:0.5} NotReady",
                            collateral_amount,
                        );
                    }
                },
                CollateralEvent::PreAuthResponse { status, .. } => match status {
                    PreAuthStatus::Approved { .. } => {
                        tracing::info!("PreAuthResponse Event: Approved");
                        flag_mock_atomic_bool(&preauth_approved_set);
                    }
                    PreAuthStatus::NotEnoughFunds => {
                        tracing::info!("PreAuthResponse Event: NotEnoughFunds");
                    }
                },
                CollateralEvent::ConfirmResponse { status, .. } => match status {
                    ConfirmStatus::Authorized => {
                        tracing::info!("ConfirmRespnse Event: Authorized");
                        flag_mock_atomic_bool(&confirm_auth_set);
                    }
                    ConfirmStatus::NotEnoughFunds => {
                        tracing::info!("ConfirmRespnse Event: NotEnoughFunds");
                    }
                },
            });

        let collateral_management = CollateralManagement {
            chain_id: 1,
            address: get_mock_address_1(),
            client_order_id: "C-1".into(),
            side: Side::Buy,
            collateral_amount: dec!(1000.0),
            asset_requirements: HashMap::from([
                (get_mock_asset_name_1(), dec!(800.0)),
                (get_mock_asset_name_2(), dec!(200.0)),
            ]),
            tracing_data: TracingData::default(),
        };

        collateral_manager
            .write()
            .unwrap()
            .handle_deposit(&host, 1, get_mock_address_1(), dec!(1000.0), timestamp)
            .unwrap();

        collateral_manager
            .write()
            .unwrap()
            .manage_collateral(collateral_management);

        collateral_manager
            .write()
            .unwrap()
            .process_collateral(&host, timestamp)
            .unwrap();

        run_mock_deferred(&rx);

        {
            let locked = collateral_manager.write().unwrap();
            let pos = locked.get_position(1, &get_mock_address_1()).unwrap();
            let pos_read = pos.read();

            assert_decimal_approx_eq!(
                pos_read.side_cr.unconfirmed_balance,
                dec!(0.0),
                zero_threshold
            );
            assert_decimal_approx_eq!(pos_read.side_cr.ready_balance, dec!(999.0), zero_threshold);
            assert_decimal_approx_eq!(pos_read.side_cr.preauth_balance, dec!(0.0), zero_threshold);
            assert_decimal_approx_eq!(pos_read.side_cr.spent_balance, dec!(1.0), zero_threshold);
            assert_eq!(pos_read.side_cr.open_lots.len(), 1);
            assert!(pos_read.side_cr.closed_lots.is_empty());

            let first = &pos_read.side_cr.open_lots[0];
            assert_decimal_approx_eq!(first.unconfirmed_amount, dec!(0.0), zero_threshold);
            assert_decimal_approx_eq!(first.ready_amount, dec!(999.0), zero_threshold);
            assert_decimal_approx_eq!(first.preauth_amount, dec!(0.0), zero_threshold);
            assert_decimal_approx_eq!(first.spent_amount, dec!(1.0), zero_threshold);

            assert_decimal_approx_eq!(
                pos_read.side_dr.unconfirmed_balance,
                dec!(0.0),
                zero_threshold
            );
            assert_decimal_approx_eq!(pos_read.side_dr.ready_balance, dec!(0.0), zero_threshold);
            assert_decimal_approx_eq!(pos_read.side_dr.preauth_balance, dec!(0.0), zero_threshold);
            assert_decimal_approx_eq!(pos_read.side_dr.spent_balance, dec!(0.0), zero_threshold);
            assert!(pos_read.side_dr.open_lots.is_empty());
            assert!(pos_read.side_dr.closed_lots.is_empty());
        }

        collateral_manager
            .write()
            .unwrap()
            .preauth_payment(
                &host,
                1,
                &get_mock_address_1(),
                &"C-1".into(),
                timestamp,
                Side::Buy,
                dec!(999.0),
            )
            .unwrap();

        assert!(test_mock_atomic_bool(&preauth_approved_get));

        {
            let locked = collateral_manager.write().unwrap();
            let pos = locked.get_position(1, &get_mock_address_1()).unwrap();
            let pos_read = pos.read();

            assert_decimal_approx_eq!(
                pos_read.side_cr.unconfirmed_balance,
                dec!(0.0),
                zero_threshold
            );
            assert_decimal_approx_eq!(pos_read.side_cr.ready_balance, dec!(0.0), zero_threshold);
            assert_decimal_approx_eq!(
                pos_read.side_cr.preauth_balance,
                dec!(999.0),
                zero_threshold
            );
            assert_decimal_approx_eq!(pos_read.side_cr.spent_balance, dec!(1.0), zero_threshold);
            assert_eq!(pos_read.side_cr.open_lots.len(), 1);
            assert!(pos_read.side_cr.closed_lots.is_empty());

            let first = &pos_read.side_cr.open_lots[0];
            assert_decimal_approx_eq!(first.unconfirmed_amount, dec!(0.0), zero_threshold);
            assert_decimal_approx_eq!(first.ready_amount, dec!(0.0), zero_threshold);
            assert_decimal_approx_eq!(first.preauth_amount, dec!(999.0), zero_threshold);
            assert_decimal_approx_eq!(first.spent_amount, dec!(1.0), zero_threshold);
        }

        collateral_manager
            .write()
            .unwrap()
            .confirm_payment(
                1,
                &get_mock_address_1(),
                &"C-1".into(),
                &"P-1".into(),
                timestamp,
                Side::Buy,
                dec!(995.0),
            )
            .unwrap();

        test_mock_atomic_bool(&confirm_auth_get);

        {
            let locked = collateral_manager.write().unwrap();
            let pos = locked.get_position(1, &get_mock_address_1()).unwrap();
            let pos_read = pos.read();

            assert_decimal_approx_eq!(
                pos_read.side_cr.unconfirmed_balance,
                dec!(0.0),
                zero_threshold
            );
            assert_decimal_approx_eq!(pos_read.side_cr.ready_balance, dec!(0.0), zero_threshold);
            assert_decimal_approx_eq!(pos_read.side_cr.preauth_balance, dec!(4.0), zero_threshold);
            assert_decimal_approx_eq!(pos_read.side_cr.spent_balance, dec!(996.0), zero_threshold);
            assert_eq!(pos_read.side_cr.open_lots.len(), 1);
            assert!(pos_read.side_cr.closed_lots.is_empty());

            let first = &pos_read.side_cr.open_lots[0];
            assert_decimal_approx_eq!(first.unconfirmed_amount, dec!(0.0), zero_threshold);
            assert_decimal_approx_eq!(first.ready_amount, dec!(0.0), zero_threshold);
            assert_decimal_approx_eq!(first.preauth_amount, dec!(4.0), zero_threshold);
            assert_decimal_approx_eq!(first.spent_amount, dec!(996.0), zero_threshold);
        }
    }
}
