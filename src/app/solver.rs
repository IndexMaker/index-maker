use std::{
    sync::{Arc, RwLock as ComponentLock},
    thread,
};

use crate::{
    app::{
        basket_manager::BasketManagerConfig,
        batch_manager::{self, BatchManagerConfig},
        collateral_manager::CollateralManagerConfig,
        index_order_manager::{self, IndexOrderManagerConfig},
        market_data::MarketDataConfig,
        order_sender::OrderSenderConfig,
        quote_request_manager::{self, QuoteRequestManagerConfig},
        simple_solver::SimpleSolverConfig,
    },
    blockchain::chain_connector::{ChainConnector, ChainNotification},
    collateral::{
        collateral_manager::CollateralEvent,
        collateral_router::{CollateralRouterEvent, CollateralTransferEvent},
    },
    index::basket_manager::BasketNotification,
    server::server::{Server, ServerEvent},
    solver::{
        batch_manager::BatchEvent, index_order_manager::{IndexOrderEvent, IndexOrderManager}, index_quote_manager::{QuoteRequestEvent, QuoteRequestManager}, solver::{OrderIdProvider, Solver}
    },
};

use super::config::ConfigBuildError;
use chrono::TimeDelta;
use crossbeam::{
    channel::{unbounded, Sender},
    select,
};
use derive_builder::Builder;
use eyre::{eyre, Result};
use parking_lot::RwLock;
use symm_core::{
    core::{
        bits::Amount,
        functional::{IntoObservableManyArc, IntoObservableSingle, IntoObservableSingleArc, IntoObservableSingleVTable, IntoObservableSingleFun},
    },
    market_data::{
        market_data_connector::MarketDataEvent, order_book::order_book_manager::OrderBookEvent,
        price_tracker::PriceEvent,
    },
    order_sender::{
        inventory_manager::InventoryEvent,
        order_connector::OrderConnectorNotification,
        order_tracker::OrderTrackerNotification,
    },
};
use tokio::sync::oneshot;

#[derive(Builder)]
#[builder(
    pattern = "owned",
    build_fn(name = "try_build", error = "ConfigBuildError")
)]
pub struct SolverConfig {
    #[builder(setter(into, strip_option))]
    pub with_strategy: SimpleSolverConfig,

    #[builder(setter(into, strip_option))]
    pub with_order_ids: Arc<RwLock<dyn OrderIdProvider + Send + Sync>>,

    #[builder(setter(into, strip_option))]
    pub with_basket_manager: BasketManagerConfig,

    #[builder(setter(into, strip_option))]
    pub with_market_data: MarketDataConfig,

    #[builder(setter(into, strip_option))]
    pub with_chain_connector: Option<Arc<ComponentLock<dyn ChainConnector + Send + Sync>>>,
    
    #[builder(setter(into, strip_option))]
    pub with_server: Option<Arc<RwLock<dyn Server + Send + Sync>>>,

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

    #[builder(setter(skip))]
    pub(crate) solver: Option<Arc<ComponentLock<Solver>>>,

    #[builder(setter(skip))]
    pub(crate) stopping: Option<(Sender<()>, oneshot::Receiver<()>)>,
}

impl SolverConfig {
    #[must_use]
    pub fn builder() -> SolverConfigBuilder {
        SolverConfigBuilder::default()
    }

    pub async fn run(&mut self) -> Result<()> {
        let (stop_tx, stop_rx) = unbounded::<()>();
        let (stopped_tx, stopped_rx) = oneshot::channel();

        let (market_data_tx, market_data_rx) = unbounded::<Arc<MarketDataEvent>>();
        let (price_event_tx, price_event_rx) = unbounded::<PriceEvent>();
        let (book_event_tx, book_event_rx) = unbounded::<OrderBookEvent>();
        let (order_event_tx, order_event_rx) = unbounded::<OrderConnectorNotification>();
        let (tracking_event_tx, tracking_event_rx) = unbounded::<OrderTrackerNotification>();
        let (inventory_event_tx, inventory_event_rx) = unbounded::<InventoryEvent>();

        let (basket_event_tx, basket_event_rx) = unbounded::<BasketNotification>();
        let (index_event_tx, index_event_rx) = unbounded::<IndexOrderEvent>();
        let (quote_event_tx, quote_event_rx) = unbounded::<QuoteRequestEvent>();
        let (transfer_event_tx, transfer_event_rx) = unbounded::<CollateralTransferEvent>();
        let (collateral_event_tx, collateral_event_rx) = unbounded::<CollateralEvent>();
        let (batch_event_tx, batch_event_rx) = unbounded::<BatchEvent>();

        // TODO: Add these
        let (chain_event_tx, chain_event_rx) = unbounded::<ChainNotification>();
        let (server_event_tx, server_event_rx) = unbounded::<ServerEvent>();
        let (router_event_tx, router_event_rx) = unbounded::<CollateralRouterEvent>();

        let market_data = self.with_market_data.market_data.clone().unwrap();
        let price_tracker = self.with_market_data.price_tracker.clone().unwrap();
        let book_manager = self.with_market_data.book_manager.clone().unwrap();

        let order_sender = self.with_order_sender.order_sender.clone().unwrap();
        let order_tracker = self.with_order_sender.order_tracker.clone().unwrap();
        let inventory_manager = self.with_order_sender.inventory_manager.clone().unwrap();

        let basket_manager = self.with_basket_manager.basket_manager.clone().unwrap();
        let batch_manager = self.with_batch_manager.batch_manager.clone().unwrap();

        let index_order_manager = self
            .with_index_order_manager
            .index_order_manager
            .clone()
            .unwrap();
        let quote_request_manager = self
            .with_quote_request_manager
            .quote_request_manager
            .clone()
            .unwrap();

        let collateral_manager = self
            .with_collateral_manager
            .collateral_manager
            .clone()
            .unwrap();
        let collateral_router = self
            .with_collateral_manager
            .collateral_router
            .clone()
            .unwrap();

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

        basket_manager
            .write()
            .get_single_observer_mut()
            .set_observer_from(basket_event_tx);

        index_order_manager
            .write()
            .map_err(|err| eyre!("Failed to obtain lock on index order manager: {:?}", err))?
            .get_single_observer_mut()
            .set_observer_from(index_event_tx);

        quote_request_manager
            .write()
            .map_err(|err| eyre!("Failed to obtain lock on quote request manager: {:?}", err))?
            .get_single_observer_mut()
            .set_observer_from(quote_event_tx);

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

        batch_manager
            .write()
            .map_err(|err| eyre!("Failed to obtain lock on batch manager: {:?}", err))?
            .get_single_observer_mut()
            .set_observer_from(batch_event_tx);

        thread::spawn(move || loop {
            select! {
                recv(stop_rx) -> _ => {
                    if let Err(err) = stopped_tx.send(()) {
                        tracing::warn!("Failed to send stopped event: {:?}", err);
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
                        tracing::debug!("Price event: {:?}", event);
                    }
                    Err(err) => {
                        tracing::warn!("Failed to receive price event: {:?}", err);
                    }
                },
                recv(book_event_rx) -> res => match res {
                    Ok(event) => {
                        tracing::debug!("Book event: {:?}", event);
                    }
                    Err(err) => {
                        tracing::warn!("Failed to receive book event: {:?}", err);
                    }
                },
                recv(order_event_rx) -> res => match res {
                    Ok(event) => {
                        tracing::debug!("Order execution event: {:?}", event);
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
                        tracing::debug!("Order tracking event: {:?}", event);
                        if let Err(err) = inventory_manager.write().handle_fill_report(event) {
                            tracing::warn!("Failed to handle order tracking event: {:?}", err);
                        }
                    },
                    Err(err) => {
                        tracing::warn!("Failed to receive order tracking event: {:?}", err);
                    }
                },
                recv(inventory_event_rx) -> res => match res {
                    Ok(event) => {
                        tracing::debug!("Inventory event: {:?}", event);
                    }
                    Err(err) => {
                        tracing::warn!("Failed to receive inventory event: {:?}", err);
                    }
                },
                recv(basket_event_rx) -> res => match res {
                    Ok(event) => {
                        tracing::debug!("Basket event");
                    }
                    Err(err) => {
                        tracing::warn!("Failed to receive basket event: {:?}", err);
                    }
                },
                recv(index_event_rx) -> res => match res {
                    Ok(event) => {
                        tracing::debug!("Index order event");
                    }
                    Err(err) => {
                        tracing::warn!("Failed to receive index order event: {:?}", err);
                    }
                },
                recv(quote_event_rx) -> res => match res {
                    Ok(event) => {
                        tracing::debug!("Quote request event");
                    }
                    Err(err) => {
                        tracing::warn!("Failed to receive quote request event: {:?}", err);
                    }
                },
                recv(transfer_event_rx) -> res => match res {
                    Ok(event) => {
                        tracing::debug!("Collateral transfer event");
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
                recv(collateral_event_rx) -> res => match res {
                    Ok(event) => {
                        tracing::debug!("Collateral event");
                    }
                    Err(err) => {
                        tracing::warn!("Failed to receive collateral event: {:?}", err);
                    }
                },
                recv(batch_event_rx) -> res => match res {
                    Ok(event) => {
                        tracing::debug!("Batch event");
                    }
                    Err(err) => {
                        tracing::warn!("Failed to receive batch event: {:?}", err);
                    }
                },
            }
        });

        self.with_market_data
            .start()
            .expect("Failed to start market data");

        self.with_order_sender
            .start()
            .expect("Failed to start order sender");

        self.stopping.replace((stop_tx, stopped_rx));

        Ok(())
    }

    pub async fn stop(&mut self) -> Result<()> {
        if let Some((stop_tx, stopped_rx)) = self.stopping.take() {
            let market_data = self.with_market_data.market_data.clone().unwrap();
            let order_sender = self.with_order_sender.order_sender.clone().unwrap();

            order_sender.write().stop().await.unwrap();

            stop_tx.send(()).unwrap();
            stopped_rx.await.unwrap();

            market_data.write().stop().await.unwrap();

            Ok(())
        } else {
            Err(eyre!("Cannot stop: Not started"))
        }
    }
}

impl SolverConfigBuilder {
    pub fn build(self) -> Result<SolverConfig, ConfigBuildError> {
        let mut config = self.try_build()?;

        let strategy =
            config.with_strategy.simple_solver.clone().ok_or_else(|| {
                ConfigBuildError::UninitializedField("with_strategy.simple_solver")
            })?;

        let basket_manager = config
            .with_basket_manager
            .basket_manager
            .clone()
            .ok_or_else(|| {
                ConfigBuildError::UninitializedField("with_basket_manager.basket_manager")
            })?;

        let price_tracker = config
            .with_market_data
            .price_tracker
            .clone()
            .ok_or_else(|| {
                ConfigBuildError::UninitializedField("with_market_data.price_tracker")
            })?;

        let order_book_manager = config
            .with_market_data
            .book_manager
            .clone()
            .ok_or_else(|| ConfigBuildError::UninitializedField("with_market_data.book_manager"))?;

        let batch_manager = config
            .with_batch_manager
            .batch_manager
            .clone()
            .ok_or_else(|| {
                ConfigBuildError::UninitializedField("with_batch_manager.batch_manager")
            })?;

        let collateral_manager = config
            .with_collateral_manager
            .collateral_manager
            .clone()
            .ok_or_else(|| {
                ConfigBuildError::UninitializedField("with_collateral_manager.collateral_manager")
            })?;

        let inventory_manager = config
            .with_order_sender
            .inventory_manager
            .clone()
            .ok_or_else(|| {
                ConfigBuildError::UninitializedField("with_order_sender.inventory_manager")
            })?;

        let index_order_manager = config
            .with_index_order_manager
            .index_order_manager
            .clone()
            .ok_or_else(|| {
                ConfigBuildError::UninitializedField("with_order_sender.index_order_manager")
            })?;

        let quote_request_manager = config
            .with_quote_request_manager
            .quote_request_manager
            .clone()
            .ok_or_else(|| {
                ConfigBuildError::UninitializedField("with_order_sender.quote_request_manager")
            })?;

        config
            .solver
            .replace(Arc::new(ComponentLock::new(Solver::new(
                strategy,
                config.with_order_ids.clone(),
                basket_manager,
                price_tracker,
                order_book_manager,
                config.with_chain_connector.clone().unwrap(),
                batch_manager,
                collateral_manager,
                index_order_manager,
                quote_request_manager,
                inventory_manager,
                config.max_batch_size,
                config.zero_threshold,
                config.client_order_wait_period,
                config.client_quote_wait_period,
            ))));

        Ok(config)
    }
}
