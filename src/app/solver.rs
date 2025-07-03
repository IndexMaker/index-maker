use core::time;
use std::{
    sync::{Arc, RwLock as ComponentLock},
    thread,
};

use crate::{
    app::{
        basket_manager::BasketManagerConfig, batch_manager::BatchManagerConfig,
        collateral_manager::CollateralManagerConfig, index_order_manager::IndexOrderManagerConfig,
        market_data::MarketDataConfig, order_sender::OrderSenderConfig,
        quote_request_manager::QuoteRequestManagerConfig,
    },
    blockchain::chain_connector::{ChainConnector, ChainNotification},
    collateral::{
        collateral_manager::CollateralEvent,
        collateral_router::{CollateralRouterEvent, CollateralTransferEvent},
    },
    index::basket_manager::BasketNotification,
    server::server::ServerEvent,
    solver::{
        batch_manager::BatchEvent,
        index_order_manager::IndexOrderEvent,
        index_quote_manager::QuoteRequestEvent,
        solver::{OrderIdProvider, Solver, SolverStrategy},
    },
};

use super::config::ConfigBuildError;
use chrono::{TimeDelta, Utc};
use crossbeam::{
    channel::{unbounded, Receiver, Sender},
    select,
};
use derive_builder::Builder;
use eyre::{eyre, OptionExt, Result};
use parking_lot::RwLock;
use symm_core::{
    core::{
        bits::Amount,
        functional::{
            IntoObservableManyArc, IntoObservableManyFun, IntoObservableSingle,
            IntoObservableSingleArc, IntoObservableSingleFun,
        },
    },
    market_data::{
        market_data_connector::MarketDataEvent, order_book::order_book_manager::OrderBookEvent,
        price_tracker::PriceEvent,
    },
    order_sender::{
        inventory_manager::InventoryEvent, order_connector::OrderConnectorNotification,
        order_tracker::OrderTrackerNotification,
    },
};
use tokio::sync::oneshot;

pub trait ChainConnectorConfig {
    fn expect_chain_connector_cloned(&self)
        -> Arc<ComponentLock<dyn ChainConnector + Send + Sync>>;

    fn try_get_chain_connector_cloned(
        &self,
    ) -> Result<Arc<ComponentLock<dyn ChainConnector + Send + Sync>>>;
}

pub trait SolverStrategyConfig {
    fn expect_solver_strategy_cloned(&self) -> Arc<dyn SolverStrategy + Send + Sync>;
    fn try_get_solver_strategy_cloned(&self) -> Result<Arc<dyn SolverStrategy + Send + Sync>>;
}

pub trait OrderIdProviderConfig {
    fn expect_order_id_provider_cloned(&self) -> Arc<RwLock<dyn OrderIdProvider + Send + Sync>>;
    fn try_get_order_id_provider_cloned(
        &self,
    ) -> Result<Arc<RwLock<dyn OrderIdProvider + Send + Sync>>>;
}

#[derive(Builder)]
#[builder(
    pattern = "owned",
    build_fn(name = "try_build", error = "ConfigBuildError")
)]
pub struct SolverConfig {
    #[builder(setter(into, strip_option))]
    pub with_strategy: Arc<dyn SolverStrategyConfig + Send + Sync>,

    #[builder(setter(into, strip_option))]
    pub with_order_ids: Arc<dyn OrderIdProviderConfig + Send + Sync>,

    #[builder(setter(into, strip_option))]
    pub with_basket_manager: BasketManagerConfig,

    #[builder(setter(into, strip_option))]
    pub with_market_data: MarketDataConfig,

    #[builder(setter(into, strip_option))]
    pub with_chain_connector: Arc<dyn ChainConnectorConfig + Send + Sync>,

    #[builder(setter(into, strip_option))]
    pub with_batch_manager: BatchManagerConfig,

    #[builder(setter(into, strip_option))]
    pub with_collateral_manager: CollateralManagerConfig,

    #[builder(setter(into, strip_option))]
    pub with_index_order_manager: IndexOrderManagerConfig,

    #[builder(setter(into, strip_option))]
    pub with_quote_request_manager: QuoteRequestManagerConfig,

    #[builder(setter(into, strip_option))]
    pub with_order_sender: OrderSenderConfig,

    #[builder(setter(into, strip_option))]
    pub max_batch_size: usize,

    #[builder(setter(into, strip_option))]
    pub zero_threshold: Amount,

    #[builder(setter(into, strip_option))]
    pub client_order_wait_period: TimeDelta,

    #[builder(setter(into, strip_option))]
    pub client_quote_wait_period: TimeDelta,

    #[builder(setter(into, strip_option))]
    pub quotes_tick_interval: Option<TimeDelta>,

    #[builder(setter(into, strip_option))]
    pub solver_tick_interval: Option<TimeDelta>,

    #[builder(setter(skip))]
    pub(crate) solver: Option<Arc<Solver>>,

    #[builder(setter(skip))]
    pub(crate) stopping_market_data: Option<(Sender<()>, oneshot::Receiver<()>)>,

    #[builder(setter(skip))]
    pub(crate) stopping_backend: Option<(Sender<()>, oneshot::Receiver<()>)>,

    #[builder(setter(skip))]
    pub(crate) stopping_quotes: Option<(Sender<()>, oneshot::Receiver<()>)>,

    #[builder(setter(skip))]
    pub(crate) stopping_solver: Option<(Sender<()>, oneshot::Receiver<()>)>,

    #[builder(setter(skip))]
    pub(crate) inventory_event_rx: Option<Receiver<InventoryEvent>>,

    #[builder(setter(skip))]
    pub(crate) collateral_event_rx: Option<Receiver<CollateralEvent>>,

    #[builder(setter(skip))]
    pub(crate) index_event_rx: Option<Receiver<IndexOrderEvent>>,
}

impl SolverConfig {
    #[must_use]
    pub fn builder() -> SolverConfigBuilder {
        SolverConfigBuilder::default()
    }

    async fn run_market_data(&mut self) -> Result<()> {
        let solver_market_data_clone = self.solver.clone().ok_or_eyre("Failed to get solver")?;

        let (stop_market_data_tx, stop_market_data_rx) = unbounded::<()>();
        let (market_data_stopped_tx, market_data_stopped_rx) = oneshot::channel();

        let (market_data_tx, market_data_rx) = unbounded::<Arc<MarketDataEvent>>();
        let (price_event_tx, price_event_rx) = unbounded::<PriceEvent>();
        let (book_event_tx, book_event_rx) = unbounded::<OrderBookEvent>();

        let market_data = self.with_market_data.try_get_market_data_cloned()?;
        let price_tracker = self.with_market_data.try_get_price_tracker_cloned()?;
        let book_manager = self.with_market_data.try_get_book_manager_cloned()?;

        price_tracker
            .write()
            .get_single_observer_mut()
            .set_observer_from(price_event_tx);

        book_manager
            .write()
            .get_single_observer_mut()
            .set_observer_from(book_event_tx);

        market_data
            .write()
            .get_multi_observer_arc()
            .write()
            .add_observer_fn(move |event: &Arc<MarketDataEvent>| {
                if let Err(err) = market_data_tx.send(event.clone()) {
                    tracing::warn!("Failed to send market data event: {:?}", err);
                }
            });

        thread::spawn(move || {
            tracing::info!("Market data started");
            loop {
                select! {
                    recv(stop_market_data_rx) -> _ => {
                        if let Err(err) = market_data_stopped_tx.send(()) {
                            tracing::warn!("Failed to send market data stopped event: {:?}", err);
                        }
                        break;
                    },
                    recv(market_data_rx) -> res => match res {
                        Ok(event) => {
                            price_tracker.write().handle_market_data(&*event);
                            book_manager.write().handle_market_data(&*event);
                        }
                        Err(err) => {
                            tracing::warn!("Failed to receive market data event: {:?}", err);
                        }
                    },
                    recv(price_event_rx) -> res => match res {
                        Ok(event) => {
                            tracing::trace!("Price event: {:?}", event);
                            solver_market_data_clone.handle_price_event(event);
                        }
                        Err(err) => {
                            tracing::warn!("Failed to receive price event: {:?}", err);
                        }
                    },
                    recv(book_event_rx) -> res => match res {
                        Ok(event) => {
                            tracing::trace!("Book event: {:?}", event);
                            solver_market_data_clone.handle_book_event(event);
                        }
                        Err(err) => {
                            tracing::warn!("Failed to receive book event: {:?}", err);
                        }
                    },
                }
            }
            tracing::info!("Market data exited");
        });

        self.with_market_data.start()?;

        self.stopping_market_data
            .replace((stop_market_data_tx, market_data_stopped_rx));

        Ok(())
    }

    async fn stop_market_data(&mut self) -> Result<()> {
        if let Some((stop_market_data_tx, market_data_stopped_rx)) =
            self.stopping_market_data.take()
        {
            let market_data = self.with_market_data.try_get_market_data_cloned()?;

            stop_market_data_tx.send(()).unwrap();
            market_data_stopped_rx.await.unwrap();

            market_data.write().stop().await.unwrap();

            Ok(())
        } else {
            Err(eyre!("Cannot stop market data: Not started"))
        }
    }

    async fn run_orders_backend(&mut self) -> Result<()> {
        let (stop_backend_tx, stop_backend_rx) = unbounded::<()>();
        let (backend_stopped_tx, backend_stopped_rx) = oneshot::channel();

        let (order_event_tx, order_event_rx) = unbounded::<OrderConnectorNotification>();
        let (tracking_event_tx, tracking_event_rx) = unbounded::<OrderTrackerNotification>();
        let (inventory_event_tx, inventory_event_rx) = unbounded::<InventoryEvent>();

        let (router_event_tx, router_event_rx) = unbounded::<CollateralRouterEvent>();
        let (transfer_event_tx, transfer_event_rx) = unbounded::<CollateralTransferEvent>();
        let (collateral_event_tx, collateral_event_rx) = unbounded::<CollateralEvent>();

        let (server_order_tx, server_order_rx) = unbounded::<Arc<ServerEvent>>();
        let (index_event_tx, index_event_rx) = unbounded::<IndexOrderEvent>();

        let order_sender = self.with_order_sender.try_get_order_sender_cloned()?;
        let order_tracker = self.with_order_sender.try_get_order_tracker_cloned()?;
        let inventory_manager = self.with_order_sender.try_get_inventory_manager_cloned()?;

        let order_server = self
            .with_index_order_manager
            .with_server
            .try_get_server_cloned()?;

        let index_order_manager = self
            .with_index_order_manager
            .try_get_index_order_manager_cloned()?;

        let collateral_manager = self
            .with_collateral_manager
            .try_get_collateral_manager_cloned()?;

        let collateral_router = self
            .with_collateral_manager
            .with_router
            .try_get_collateral_router_cloned()?;

        let collateral_bridges = collateral_router
            .read()
            .map_err(|err| {
                eyre!(
                    "Failed to obtain lock on collateral router manager: {:?}",
                    err
                )
            })?
            .get_bridges();

        for bridge in collateral_bridges {
            bridge
                .write()
                .map_err(|err| {
                    eyre!(
                        "Failed to obtain lock on collateral bridge manager: {:?}",
                        err
                    )
                })?
                .set_observer_from(router_event_tx.clone());
        }

        order_sender
            .write()
            .get_single_observer_arc()
            .write()
            .set_observer_from(order_event_tx);

        order_tracker
            .write()
            .get_single_observer_mut()
            .set_observer_from(tracking_event_tx);

        inventory_manager
            .write()
            .get_single_observer_mut()
            .set_observer_from(inventory_event_tx);

        index_order_manager
            .write()
            .map_err(|err| eyre!("Failed to obtain lock on index order manager: {:?}", err))?
            .get_single_observer_mut()
            .set_observer_from(index_event_tx);

        collateral_manager
            .write()
            .map_err(|err| eyre!("Failed to obtain lock on collateral manager: {:?}", err))?
            .get_single_observer_mut()
            .set_observer_from(collateral_event_tx);

        collateral_router
            .write()
            .map_err(|err| eyre!("Failed to obtain lock on collateral router: {:?}", err))?
            .get_single_observer_mut()
            .set_observer_from(transfer_event_tx);

        order_server.write().add_observer_from(server_order_tx);

        thread::spawn(move || {
            tracing::info!("Backend started");
            loop {
                select! {
                    recv(stop_backend_rx) -> _ => {
                        if let Err(err) = backend_stopped_tx.send(()) {
                            tracing::warn!("Failed to send backend stopped event: {:?}", err);
                        }
                        break;
                    },
                    recv(order_event_rx) -> res => match res {
                        Ok(event) => {
                            tracing::trace!("Order execution event: {:?}", event);
                            if let Err(err) = order_tracker.write().handle_order_notification(event) {
                                tracing::warn!("Failed to handle order execution event: {:?}", err);
                            }
                        },
                        Err(err) => {
                            tracing::warn!("Failed to receive order execution event: {:?}", err);
                        },
                    },
                    recv(tracking_event_rx) -> res => match res {
                        Ok(event) => {
                            tracing::trace!("Order tracking event: {:?}", event);
                            if let Err(err) = inventory_manager.write().handle_fill_report(event) {
                                tracing::warn!("Failed to handle order tracking event: {:?}", err);
                            }
                        },
                        Err(err) => {
                            tracing::warn!("Failed to receive order tracking event: {:?}", err);
                        }
                    },
                    recv(router_event_rx) -> res => match res {
                        Ok(event) => {
                            tracing::trace!("Router event");
                            match collateral_router.write() {
                                Ok(mut router) => if let Err(err) = router.handle_collateral_router_event(event) {
                                    tracing::warn!("Failed to handle collateral router event: {:?}", err);
                                }
                                Err(err) => {
                                    tracing::warn!("Failed to obtain lock on collateral router: {:?}", err);
                                }
                            }
                        }
                        Err(err) => {
                            tracing::warn!("Failed to receive router event: {:?}", err);
                        }
                    },
                    recv(transfer_event_rx) -> res => match res {
                        Ok(event) => {
                            tracing::trace!("Collateral transfer event");
                            match collateral_manager.write() {
                                Ok(mut collateral_manager) => {
                                    if let Err(err) = collateral_manager.handle_collateral_transfer_event(event) {
                                        tracing::warn!("Failed to handle collateral transfer event: {:?}", err);
                                    }
                                },
                                Err(err) => {
                                    tracing::warn!("Failed to obtain lock on collateral manager: {:?}", err);
                                }
                            };
                        },
                        Err(err) => {
                            tracing::warn!("Failed to receive collateral transfer event: {:?}", err);
                        }
                    },
                    recv(server_order_rx) -> res => match res {
                        Ok(event) => {
                            tracing::trace!("Server order event");
                            match index_order_manager.write() {
                                Ok(mut manager) => if let Err(err) = manager.handle_server_message(&*event) {
                                        tracing::warn!("Failed to handle index order event: {:?}", err);
                                }
                                Err(err) => {
                                    tracing::warn!("Failed to obtain lock on index order manager: {:?}", err);
                                }
                            }
                        },
                        Err(err) => {
                            tracing::warn!("Failed to receive server order event: {:?}", err);
                        }
                    },
                }
            }
            tracing::info!("Backend exited");
        });

        self.with_order_sender.start()?;

        self.stopping_backend
            .replace((stop_backend_tx, backend_stopped_rx));

        self.index_event_rx.replace(index_event_rx);
        self.inventory_event_rx.replace(inventory_event_rx);
        self.collateral_event_rx.replace(collateral_event_rx);

        Ok(())
    }

    async fn stop_orders_backend(&mut self) -> Result<()> {
        if let Some((stop_backend_tx, backend_stopped_rx)) = self.stopping_backend.take() {
            let order_sender = self.with_order_sender.try_get_order_sender_cloned()?;
            order_sender.write().stop().await.unwrap();

            stop_backend_tx.send(()).unwrap();
            backend_stopped_rx.await.unwrap();

            Ok(())
        } else {
            Err(eyre!("Cannot stop backend: Not started"))
        }
    }

    pub async fn run_quotes_backend(&mut self) -> Result<()> {
        let solver_quotes_clone = self.solver.clone().ok_or_eyre("Failed to get solver")?;

        let (stop_quotes_tx, stop_quotes_rx) = unbounded::<()>();
        let (quotes_stopped_tx, quotes_stopped_rx) = oneshot::channel();

        let (quote_event_tx, quote_event_rx) = unbounded::<QuoteRequestEvent>();
        let (server_quote_tx, server_quote_rx) = unbounded::<Arc<ServerEvent>>();

        let tick_delta = time::Duration::from_millis(
            self.quotes_tick_interval
                .map(|d| d.num_milliseconds())
                .unwrap_or_default() as u64,
        );

        let quote_server = self
            .with_quote_request_manager
            .with_server
            .try_get_server_cloned()?;

        let quote_request_manager = self
            .with_quote_request_manager
            .try_get_quote_request_manager_cloned()?;

        quote_request_manager
            .write()
            .map_err(|err| eyre!("Failed to obtain lock on quote request manager: {:?}", err))?
            .get_single_observer_mut()
            .set_observer_from(quote_event_tx);

        quote_server.write().add_observer_from(server_quote_tx);

        thread::spawn(move || {
            tracing::info!("Quotes started");
            loop {
                select! {
                    recv(stop_quotes_rx) -> _ => {
                        if let Err(err) = quotes_stopped_tx.send(()) {
                            tracing::warn!("Failed to send quotes stopped event: {:?}", err);
                        }
                        break;
                    },
                    recv(server_quote_rx) -> res => match res {
                        Ok(event) => {
                            tracing::trace!("Server quote event");
                            match quote_request_manager.write() {
                                Ok(mut manager) => if let Err(err) = manager.handle_server_message(&*event) {
                                        tracing::warn!("Failed to handle index order event: {:?}", err);
                                }
                                Err(err) => {
                                    tracing::warn!("Failed to obtain lock on index order manager: {:?}", err);
                                }
                            }
                        },
                        Err(err) => {
                            tracing::warn!("Failed to receive server quote event: {:?}", err);
                        }
                    },
                    recv(quote_event_rx) -> res => match res {
                        Ok(event) => {
                            tracing::trace!("Quote request event");
                            if let Err(err) = solver_quotes_clone.handle_quote_request(event) {
                                tracing::warn!("Failed to handle quote request event: {:?}", err);
                            }
                        }
                        Err(err) => {
                            tracing::warn!("Failed to receive quote request event: {:?}", err);
                        }
                    },
                    default(tick_delta) => {
                        solver_quotes_clone.solve_quotes(Utc::now());
                    }
                }
            }
            tracing::info!("Quotes exited");
        });

        self.stopping_quotes
            .replace((stop_quotes_tx, quotes_stopped_rx));

        Ok(())
    }

    async fn stop_quotes_backend(&mut self) -> Result<()> {
        if let Some((stop_quotes_tx, quotes_stopped_rx)) = self.stopping_quotes.take() {
            stop_quotes_tx.send(()).unwrap();
            quotes_stopped_rx.await.unwrap();

            Ok(())
        } else {
            Err(eyre!("Cannot stop quotes: Not started"))
        }
    }

    pub async fn run_solver(&mut self) -> Result<()> {
        let solver = self.solver.clone().ok_or_eyre("Failed to get solver")?;

        let (stop_solver_tx, stop_solver_rx) = unbounded::<()>();
        let (solver_stopped_tx, solver_stopped_rx) = oneshot::channel();

        let (basket_event_tx, basket_event_rx) = unbounded::<BasketNotification>();

        let (batch_event_tx, batch_event_rx) = unbounded::<BatchEvent>();
        let (chain_event_tx, chain_event_rx) = unbounded::<ChainNotification>();

        let basket_manager = self.with_basket_manager.try_get_basket_manager_cloned()?;
        let batch_manager = self.with_batch_manager.try_get_batch_manager_cloned()?;

        let chain_connector = self.with_chain_connector.try_get_chain_connector_cloned()?;

        basket_manager
            .write()
            .get_single_observer_mut()
            .set_observer_from(basket_event_tx);

        batch_manager
            .write()
            .map_err(|err| eyre!("Failed to obtain lock on batch manager: {:?}", err))?
            .get_single_observer_mut()
            .set_observer_from(batch_event_tx);

        chain_connector
            .write()
            .map_err(|err| eyre!("Failed to obtain lock on chain connector: {:?}", err))?
            .set_observer_from(chain_event_tx);

        let tick_delta = time::Duration::from_millis(
            self.solver_tick_interval
                .map(|d| d.num_milliseconds())
                .unwrap_or_default() as u64,
        );

        let collateral_event_rx = self
            .collateral_event_rx
            .take()
            .ok_or_eyre("Failed to obtain collateral event receiver")?;

        let inventory_event_rx = self
            .inventory_event_rx
            .take()
            .ok_or_eyre("Failed to obtain inventory event receiver")?;

        let index_event_rx = self
            .index_event_rx
            .take()
            .ok_or_eyre("Failed to obtain index order event receiver")?;

        thread::spawn(move || {
            tracing::info!("Solver started");
            loop {
                select! {
                    recv(stop_solver_rx) -> _ => {
                        if let Err(err) = solver_stopped_tx.send(()) {
                            tracing::warn!("Failed to send solver stopped event: {:?}", err);
                        }
                        break;
                    },
                    recv(collateral_event_rx) -> res => match res {
                        Ok(event) => {
                            tracing::trace!("Collateral event");
                            if let Err(err) = solver.handle_collateral_event(event) {
                                tracing::warn!("Failed to handle collateral event: {:?}", err);
                            }
                        }
                        Err(err) => {
                            tracing::warn!("Failed to receive collateral event: {:?}", err);
                        }
                    },
                    recv(batch_event_rx) -> res => match res {
                        Ok(event) => {
                            tracing::trace!("Batch event");
                            if let Err(err) = solver.handle_batch_event(event) {
                                tracing::warn!("Failed to handle batch event: {:?}", err);
                            }
                        }
                        Err(err) => {
                            tracing::warn!("Failed to receive batch event: {:?}", err);
                        }
                    },
                    recv(inventory_event_rx) -> res => match res {
                        Ok(event) => {
                            tracing::trace!("Inventory event: {:?}", event);
                            if let Err(err) = solver.handle_inventory_event(event) {
                                tracing::warn!("Failed to handle inventory event: {:?}", err);
                            }
                        }
                        Err(err) => {
                            tracing::warn!("Failed to receive inventory event: {:?}", err);
                        }
                    },
                    recv(basket_event_rx) -> res => match res {
                        Ok(event) => {
                            if let Err(err) = solver.handle_basket_event(event) {
                                tracing::warn!("Failed to handle basket event: {:?}", err);
                            }
                        }
                        Err(err) => {
                            tracing::warn!("Failed to receive basket event: {:?}", err);
                        }
                    },
                    recv(index_event_rx) -> res => match res {
                        Ok(event) => {
                            tracing::trace!("Index order event");
                            if let Err(err) = solver.handle_index_order(event) {
                                tracing::warn!("Failed to handle index order event: {:?}", err);
                            }
                        }
                        Err(err) => {
                            tracing::warn!("Failed to receive index order event: {:?}", err);
                        }
                    },
                    recv(chain_event_rx) -> res => match res {
                        Ok(event) => {
                            tracing::trace!("Chain event");
                            if let Err(err) = solver.handle_chain_event(event) {
                                tracing::warn!("Failed to handle chain event: {:?}", err);
                            }
                        },
                        Err(err) => {
                            tracing::warn!("Failed to receive chain event: {:?}", err);
                        }
                    },
                    default(tick_delta) => {
                        solver.solve(Utc::now());
                    }
                }
            }
            tracing::info!("Solver exited");
        });

        self.stopping_solver
            .replace((stop_solver_tx, solver_stopped_rx));

        Ok(())
    }
    
    pub async fn run_quotes_solver(&mut self) -> Result<()> {
        let solver = self.solver.clone().ok_or_eyre("Failed to get solver")?;

        let (stop_solver_tx, stop_solver_rx) = unbounded::<()>();
        let (solver_stopped_tx, solver_stopped_rx) = oneshot::channel();

        let (basket_event_tx, basket_event_rx) = unbounded::<BasketNotification>();
        let (chain_event_tx, chain_event_rx) = unbounded::<ChainNotification>();

        let basket_manager = self.with_basket_manager.try_get_basket_manager_cloned()?;
        let chain_connector = self.with_chain_connector.try_get_chain_connector_cloned()?;

        basket_manager
            .write()
            .get_single_observer_mut()
            .set_observer_from(basket_event_tx);

        chain_connector
            .write()
            .map_err(|err| eyre!("Failed to obtain lock on chain connector: {:?}", err))?
            .set_observer_from(chain_event_tx);

        let tick_delta = time::Duration::from_millis(
            self.solver_tick_interval
                .map(|d| d.num_milliseconds())
                .unwrap_or_default() as u64,
        );

        thread::spawn(move || {
            tracing::info!("Solver started");
            loop {
                select! {
                    recv(stop_solver_rx) -> _ => {
                        if let Err(err) = solver_stopped_tx.send(()) {
                            tracing::warn!("Failed to send solver stopped event: {:?}", err);
                        }
                        break;
                    },
                    recv(basket_event_rx) -> res => match res {
                        Ok(event) => {
                            if let Err(err) = solver.handle_basket_event(event) {
                                tracing::warn!("Failed to handle basket event: {:?}", err);
                            }
                        }
                        Err(err) => {
                            tracing::warn!("Failed to receive basket event: {:?}", err);
                        }
                    },
                    recv(chain_event_rx) -> res => match res {
                        Ok(event) => {
                            tracing::trace!("Chain event");
                            if let Err(err) = solver.handle_chain_event(event) {
                                tracing::warn!("Failed to handle chain event: {:?}", err);
                            }
                        },
                        Err(err) => {
                            tracing::warn!("Failed to receive chain event: {:?}", err);
                        }
                    },
                    default(tick_delta) => {
                        solver.solve(Utc::now());
                    }
                }
            }
            tracing::info!("Solver exited");
        });

        self.stopping_solver
            .replace((stop_solver_tx, solver_stopped_rx));

        Ok(())
    }


    async fn stop_solver(&mut self) -> Result<()> {
        if let Some((stop_solver_tx, solver_stopped_rx)) = self.stopping_solver.take() {
            stop_solver_tx.send(()).unwrap();
            solver_stopped_rx.await.unwrap();

            Ok(())
        } else {
            Err(eyre!("Cannot stop solver: Not started"))
        }
    }

    pub async fn run(&mut self) -> Result<()> {
        self.run_orders_backend().await?;
        self.run_quotes_backend().await?;
        self.run_market_data().await?;
        self.run_solver().await?;
        Ok(())
    }

    pub async fn stop(&mut self) -> Result<()> {
        self.stop_orders_backend().await?;
        self.stop_quotes_backend().await?;
        self.stop_solver().await?;
        self.stop_market_data().await?;
        Ok(())
    }
    
    pub async fn run_quotes(&mut self) -> Result<()> {
        self.run_quotes_backend().await?;
        self.run_market_data().await?;
        self.run_quotes_solver().await?;
        Ok(())
    }

    pub async fn stop_quotes(&mut self) -> Result<()> {
        self.stop_quotes_backend().await?;
        self.stop_solver().await?;
        self.stop_market_data().await?;
        Ok(())
    }
}

impl SolverConfigBuilder {
    pub fn build(self) -> Result<SolverConfig, ConfigBuildError> {
        let mut config = self.try_build()?;

        let strategy = config
            .with_strategy
            .try_get_solver_strategy_cloned()
            .map_err(|_| ConfigBuildError::UninitializedField("with_strategy.simple_solver"))?;

        let order_id_provider = config
            .with_order_ids
            .try_get_order_id_provider_cloned()
            .map_err(|_| {
                ConfigBuildError::UninitializedField("with_order_ids.order_id_provider")
            })?;

        let basket_manager = config
            .with_basket_manager
            .try_get_basket_manager_cloned()
            .map_err(|_| {
                ConfigBuildError::UninitializedField("with_basket_manager.basket_manager")
            })?;

        let price_tracker = config
            .with_market_data
            .try_get_price_tracker_cloned()
            .map_err(|_| ConfigBuildError::UninitializedField("with_market_data.price_tracker"))?;

        let order_book_manager = config
            .with_market_data
            .try_get_book_manager_cloned()
            .map_err(|_| ConfigBuildError::UninitializedField("with_market_data.book_manager"))?;

        let batch_manager = config
            .with_batch_manager
            .try_get_batch_manager_cloned()
            .map_err(|_| {
                ConfigBuildError::UninitializedField("with_batch_manager.batch_manager")
            })?;

        let collateral_manager = config
            .with_collateral_manager
            .try_get_collateral_manager_cloned()
            .map_err(|_| {
                ConfigBuildError::UninitializedField("with_collateral_manager.collateral_manager")
            })?;

        let inventory_manager = config
            .with_order_sender
            .try_get_inventory_manager_cloned()
            .map_err(|_| {
                ConfigBuildError::UninitializedField("with_order_sender.inventory_manager")
            })?;

        let index_order_manager = config
            .with_index_order_manager
            .try_get_index_order_manager_cloned()
            .map_err(|_| {
                ConfigBuildError::UninitializedField("with_order_sender.index_order_manager")
            })?;

        let quote_request_manager = config
            .with_quote_request_manager
            .try_get_quote_request_manager_cloned()
            .map_err(|_| {
                ConfigBuildError::UninitializedField("with_order_sender.quote_request_manager")
            })?;

        let chain_connector = config
            .with_chain_connector
            .try_get_chain_connector_cloned()
            .map_err(|_| {
                ConfigBuildError::UninitializedField("with_chain_connector.chain_connector")
            })?;

        config.solver.replace(Arc::new(Solver::new(
            strategy,
            order_id_provider,
            basket_manager,
            price_tracker,
            order_book_manager,
            chain_connector,
            batch_manager,
            collateral_manager,
            index_order_manager,
            quote_request_manager,
            inventory_manager,
            config.max_batch_size,
            config.zero_threshold,
            config.client_order_wait_period,
            config.client_quote_wait_period,
        )));

        Ok(config)
    }
}
