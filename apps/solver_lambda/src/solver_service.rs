use std::{
    collections::VecDeque,
    sync::{Arc, RwLock as ComponentLock},
};

use chrono::TimeDelta;
use crossbeam::channel::unbounded;
use eyre::Context;
use index_core::{
    blockchain::chain_connector,
    collateral::collateral_router::{CollateralBridge, CollateralRouter},
    index::basket_manager::BasketManager,
};
use index_maker::{
    app::{
        basket_manager, batch_manager, collateral_manager, index_order_manager,
        quote_request_manager, simple_router::SimpleBridge,
    },
    collateral::collateral_manager::CollateralManager,
    solver::{
        batch_manager::BatchManager, index_order_manager::IndexOrderManager,
        index_quote_manager::QuoteRequestManager, mint_invoice_manager::MintInvoiceManager,
        solver::Solver, solvers::simple_solver::SimpleSolver,
    },
};
use itertools::Itertools;
use parking_lot::RwLock as AtomicLock;
use rust_decimal::dec;

use symm_core::{
    core::{
        bits::Symbol,
        functional::{IntoObservableSingle, IntoObservableSingleFun},
        persistence::{self, util::InMemoryPersistence, Persist, Persistence},
    },
    market_data::{
        market_data_connector::MarketDataEvent,
        order_book::order_book_manager::{self, PricePointBookManager},
        price_tracker::{self, PriceTracker},
    },
    order_sender::{
        inventory_manager::{self, InventoryManager},
        order_connector::{OrderConnectorNotification, SessionId},
        order_tracker::OrderTracker,
    },
};
use tracing::event;

use crate::{
    solver_input::SolverInput,
    solver_output::SolverOutput,
    solver_output_chain_connector::SolverOutputChainConnector,
    solver_output_order_sender::SolverOutputOrderSender,
    solver_output_server::SolverOutputServer,
    solver_state::{IndexDefinition, SolverState},
    solver_state_order_ids::SolverStateOrderIdProvider,
};

pub struct SolverService {
    pub config_path: String,
}

impl SolverService {
    pub fn new(config_path: String) -> Self {
        Self { config_path }
    }

    pub async fn solve(&self, input: SolverInput) -> eyre::Result<SolverOutput> {
        // TODO: Load this configuration

        let price_threshold = dec!(0.01);
        let max_levels = 5usize;
        let fee_factor = dec!(1.002);
        let max_order_volley_size = dec!(20.0);
        let max_volley_size = dec!(500.0);
        let min_asset_volley_size = dec!(5.0);
        let asset_volley_step_size = dec!(0.1);
        let max_total_volley_size = dec!(1000.0);
        let min_total_volley_available = dec!(200.0);

        let fill_threshold = dec!(0.9999);
        let mint_threshold = dec!(0.99);
        let mint_wait_period = TimeDelta::seconds(10);

        let max_batch_size = 16usize;
        let collateral_zero_threshold = dec!(0.000_001);
        let assets_zero_threshold = dec!(0.000_000_000_000_000_001);
        let order_expiry_after = chrono::Duration::minutes(30);
        let client_order_wait_period = TimeDelta::seconds(5);
        let client_quote_wait_period = TimeDelta::milliseconds(500);

        tracing::info!("Loading state...");

        //
        // Market data part

        let price_tracker = Arc::new(AtomicLock::new(PriceTracker::new()));
        let order_book_manager = Arc::new(AtomicLock::new(PricePointBookManager::new(
            assets_zero_threshold,
        )));

        //
        // Order sending part

        let order_sender = Arc::new(AtomicLock::new(SolverOutputOrderSender::new()));

        let order_tracker = Arc::new(AtomicLock::new(OrderTracker::new(
            order_sender.clone(),
            assets_zero_threshold,
        )));
        let (tracked_order_tx, tracked_order_rx) = unbounded();
        order_tracker
            .write()
            .get_single_observer_mut()
            .set_observer_from(tracked_order_tx);

        let inventory_manager_persistence = Arc::new(InMemoryPersistence::new());
        if let Some(inventory_state) = input.state.inventory {
            inventory_manager_persistence.store_value(inventory_state)?;
        }

        let inventory_manager = Arc::new(AtomicLock::new(InventoryManager::new(
            order_tracker.clone(),
            inventory_manager_persistence.clone(),
            assets_zero_threshold,
        )));
        let (inventory_tx, inventory_rx) = unbounded();
        inventory_manager
            .write()
            .get_single_observer_mut()
            .set_observer_from(inventory_tx);

        //
        // Chain connector part

        let chain_connector = Arc::new(ComponentLock::new(SolverOutputChainConnector::new()));

        //
        // Collateral manager part
        let collateral_router = Arc::new(AtomicLock::new(CollateralRouter::new()));
        let (collateral_router_tx, collateral_router_rx) = unbounded();
        collateral_router
            .write()
            .get_single_observer_mut()
            .set_observer_from(collateral_router_tx);

        let collateral_persistence = Arc::new(InMemoryPersistence::new());
        if let Some(collateral_state) = input.state.collateral {
            collateral_persistence.store_value(collateral_state)?;
        }

        let collateral_manager = Arc::new(ComponentLock::new(CollateralManager::new(
            collateral_router.clone(),
            collateral_persistence.clone(),
            collateral_zero_threshold,
        )));
        let (collateral_tx, collateral_rx) = unbounded();
        collateral_manager
            .write()
            .map_err(|err| eyre::eyre!("Failed to access collateral manager{:?}", err))?
            .get_single_observer_mut()
            .set_observer_from(collateral_tx);

        //
        // Server part

        let server = Arc::new(AtomicLock::new(SolverOutputServer::new()));

        //
        // Quote request manager part
        let quote_request_manager =
            Arc::new(ComponentLock::new(QuoteRequestManager::new(server.clone())));

        //
        // Invoice manager part
        let invoice_persistence = Arc::new(InMemoryPersistence::new());
        if let Some(invoice_state) = input.state.invoices {
            invoice_persistence.store_value(invoice_state)?;
        }
        let invoice_manager = Arc::new(AtomicLock::new(MintInvoiceManager::new(
            invoice_persistence.clone(),
        )));

        //
        // Index order manager part

        let index_order_persistence = Arc::new(InMemoryPersistence::new());
        if let Some(index_orders_state) = input.state.index_orders {
            index_order_persistence.store_value(index_orders_state)?;
        }
        let index_order_manager = Arc::new(ComponentLock::new(IndexOrderManager::new(
            server.clone(),
            invoice_manager,
            index_order_persistence.clone(),
            collateral_zero_threshold,
        )));
        let (index_order_tx, index_order_rx) = unbounded();
        index_order_manager
            .write()
            .map_err(|err| eyre::eyre!("Failed to access index order manager{:?}", err))?
            .get_single_observer_mut()
            .set_observer_from(index_order_tx);

        //
        // Solver part

        let order_id_provider = Arc::new(AtomicLock::new(SolverStateOrderIdProvider {
            order_ids: VecDeque::from(input.state.order_ids),
            batch_order_ids: VecDeque::from(input.state.batch_ids),
            payment_ids: VecDeque::from(input.state.payment_ids),
        }));

        let basket_manager = Arc::new(AtomicLock::new(BasketManager::new()));
        let (basket_tx, basket_rx) = unbounded();
        basket_manager
            .write()
            .get_single_observer_mut()
            .set_observer_from(basket_tx);

        let solver_strategy = Arc::new(SimpleSolver::new(
            price_threshold,
            max_levels,
            fee_factor,
            max_order_volley_size,
            max_volley_size,
            min_asset_volley_size,
            asset_volley_step_size,
            max_total_volley_size,
            min_total_volley_available,
        ));

        let batch_manager_persistence = Arc::new(InMemoryPersistence::new());
        if let Some(batch_state) = input.state.batch {
            batch_manager_persistence.store_value(batch_state)?;
        }

        let batch_manager = Arc::new(ComponentLock::new(BatchManager::new(
            batch_manager_persistence.clone(),
            max_batch_size,
            assets_zero_threshold,
            fill_threshold,
            mint_threshold,
            mint_wait_period,
        )));
        let (batch_tx, batch_rx) = unbounded();
        batch_manager
            .write()
            .map_err(|err| eyre::eyre!("Failed to access batch manager{:?}", err))?
            .get_single_observer_mut()
            .set_observer_from(batch_tx);

        let solver_persistence = Arc::new(InMemoryPersistence::new());
        if let Some(solver_state) = input.state.solver {
            solver_persistence.store_value(solver_state)?;
        }

        let solver = {
            let mut solver = Arc::new(Solver::new(
                solver_strategy,
                order_id_provider.clone(),
                basket_manager.clone(),
                price_tracker.clone(),
                order_book_manager.clone(),
                chain_connector.clone(),
                batch_manager,
                collateral_manager.clone(),
                index_order_manager.clone(),
                quote_request_manager,
                inventory_manager.clone(),
                solver_persistence.clone(),
                max_batch_size,
                collateral_zero_threshold,
                order_expiry_after,
                client_order_wait_period,
                client_quote_wait_period,
            ));

            if let Some(solver_mut) = Arc::get_mut(&mut solver) {
                // This will load all the state from InMemoryPersistence
                // that we created from input.state.
                solver_mut.load()?;
            }

            solver
        };

        // Flusher

        let flush_events = {
            let solver = solver.clone();

            move || -> eyre::Result<bool> {
                let mut any_events = false;
                loop {
                    crossbeam::select! {
                        recv(inventory_rx) -> res => {
                            res
                                .context("Failed to receive inventory event")
                                .and_then(|e| solver.handle_inventory_event(e))?;
                        }
                        recv(collateral_rx) -> res => {
                            res
                                .context("Failed to receive collateral event")
                                .and_then(|e| solver.handle_collateral_event(e))?;
                        }
                        recv(index_order_rx) -> res => {
                            res
                                .context("Failed to receive index order event")
                                .and_then(|e| solver.handle_index_order(e))?;
                        }
                        recv(basket_rx) -> res => {
                            res
                                .context("Failed to receive basket event")
                                .and_then(|e| solver.handle_basket_event(e))?;
                        }
                        recv(batch_rx) -> res => {
                            res
                                .context("Failed to receive batch event")
                                .and_then(|e| solver.handle_batch_event(e))?;
                        }

                        recv(tracked_order_rx) -> res => {
                            res
                                .context("Failed to receive tracked order event")
                                .and_then(|e| inventory_manager.write().handle_fill_report(e))?;
                        }

                        recv(collateral_router_rx) -> res => {
                            let e = res.context("Failed to receive collateral router event")?;
                            let mut c = collateral_manager.write()
                                .map_err(|err| eyre::eyre!("Failed to access collateral manager{:?}", err))?;
                            c.handle_collateral_transfer_event(e)?;
                        }

                        default => {
                            return Ok(any_events)
                        }
                    }
                    any_events = true;
                }
            }
        };

        let next_step = {
            let solver = solver.clone();

            move || -> eyre::Result<()> {
                loop {
                    // Poke solver
                    solver.solve(input.timestamp);

                    // Flush all internal events
                    if !flush_events()? {
                        return Ok(());
                    }
                }
            }
        };

        // TODO: Temporary routing filler
        {
            let route_symbol = Symbol::from("SO2");
            let chain_id = 8453;

            let simple_bridge = Arc::new(SimpleBridge::new(
                &Symbol::from("Start::USDC"),
                &route_symbol,
                &Symbol::from("End::USDC"),
            ));

            let route_start = simple_bridge.get_source().get_full_name();
            let route_end = simple_bridge.get_destination().get_full_name();

            collateral_router
                .write()
                .add_route(&[route_start.clone(), route_end.clone()])?;

            collateral_router
                .write()
                .add_bridge(simple_bridge as Arc<dyn CollateralBridge>)?;

            collateral_router.write().add_chain_source(
                chain_id,
                route_symbol,
                route_start.clone(),
            )?;

            collateral_router
                .write()
                .set_default_destination(route_end.clone())?;
        }

        // Temporary session filler
        {
            order_tracker.write().handle_order_notification(
                OrderConnectorNotification::SessionLogon {
                    session_id: SessionId::from("S1"),
                    timestamp: input.timestamp,
                },
            )?;
        }

        //
        // Feed external events

        for index_definition in input.state.indexes {
            basket_manager
                .write()
                .set_basket(&index_definition.symbol, &index_definition.basket);
        }

        next_step()?;

        for event in input.market_data_events {
            price_tracker.write().handle_market_data(&event);
            order_book_manager.write().handle_market_data(&event);
        }

        next_step()?;

        for event in input.server_events {
            index_order_manager
                .write()
                .map_err(|e| eyre::eyre!("Failed to access index order manager: {:?}", e))?
                .handle_server_message(&event)?;
        }

        next_step()?;

        for event in input.chain_events {
            solver.handle_chain_event(event)?;
        }

        next_step()?;

        for event in input.router_events {
            collateral_router
                .read()
                .handle_collateral_router_event(event)?;
        }

        next_step()?;

        for event in input.order_events {
            order_tracker.write().handle_order_notification(event)?;
        }

        next_step()?;

        tracing::info!("Storing state...");

        // This will store all the state into InMemoryPersistence
        // that we then send back as output.state.
        solver.store()?;

        let indexes = basket_manager
            .read()
            .get_baskets()
            .iter()
            .map(|(s, b)| IndexDefinition {
                symbol: s.clone(),
                basket: b.clone(),
            })
            .collect_vec();

        let order_ids = Vec::from_iter(order_id_provider.write().order_ids.drain(..));
        let batch_ids = Vec::from_iter(order_id_provider.write().batch_order_ids.drain(..));
        let payment_ids = Vec::from_iter(order_id_provider.write().payment_ids.drain(..));

        let chain_commands = Vec::from_iter(
            chain_connector
                .write()
                .map_err(|e| eyre::eyre!("Failed to access index order manager: {:?}", e))?
                .commands
                .write()
                .drain(..),
        );

        let output = SolverOutput {
            orders: Vec::from_iter(order_sender.write().orders.drain(..)),
            server_responses: Vec::from_iter(server.write().responses.drain(..)),
            chain_commands,
            state: SolverState {
                order_ids,
                batch_ids,
                payment_ids,
                indexes,
                index_orders: index_order_persistence.load_value()?,
                invoices: invoice_persistence.load_value()?,
                collateral: collateral_persistence.load_value()?,
                inventory: inventory_manager_persistence.load_value()?,
                batch: batch_manager_persistence.load_value()?,
                solver: solver_persistence.load_value()?,
            },
        };

        Ok(output)
    }
}
