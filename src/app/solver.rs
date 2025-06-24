use std::{
    sync::{Arc, RwLock as ComponentLock},
    thread,
};

use crate::{
    app::{
        basket_manager::BasketManagerConfig, batch_manager::BatchManagerConfig,
        collateral_manager::CollateralManagerConfig, market_data::MarketDataConfig,
        order_sender::OrderSenderConfig, simple_solver::SimpleSolverConfig,
    },
    blockchain::chain_connector::ChainConnector,
    solver::{
        index_order_manager::IndexOrderManager,
        index_quote_manager::QuoteRequestManager,
        solver::{OrderIdProvider, Solver},
    },
};

use super::config::ConfigBuildError;
use chrono::TimeDelta;
use crossbeam::{channel::unbounded, select};
use derive_builder::Builder;
use eyre::Result;
use parking_lot::RwLock;
use symm_core::{
    core::{
        bits::Amount,
        functional::{IntoObservableManyArc, IntoObservableSingle},
    },
    market_data::{
        market_data_connector::MarketDataEvent,
        order_book::order_book_manager::{OrderBookEvent},
        price_tracker::PriceEvent,
    },
};

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
    pub with_chain_connector: Arc<ComponentLock<dyn ChainConnector + Send + Sync>>,

    #[builder(setter(into, strip_option))]
    pub with_batch_manager: BatchManagerConfig,

    #[builder(setter(into, strip_option))]
    pub with_collateral_manager: CollateralManagerConfig,

    #[builder(setter(into, strip_option))]
    pub with_index_order_manager: Arc<ComponentLock<IndexOrderManager>>,

    #[builder(setter(into, strip_option))]
    pub with_quote_request_manager: Arc<ComponentLock<QuoteRequestManager>>,

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
}

impl SolverConfig {
    #[must_use]
    pub fn builder() -> SolverConfigBuilder {
        SolverConfigBuilder::default()
    }

    pub async fn run(self) -> Result<()> {
        let (market_data_tx, market_data_rx) = unbounded::<Arc<MarketDataEvent>>();
        let (price_event_tx, price_event_rx) = unbounded::<PriceEvent>();
        let (book_event_tx, book_event_rx) = unbounded::<OrderBookEvent>();

        let market_data = self.with_market_data.market_data.unwrap();
        let price_tracker = self.with_market_data.price_tracker.unwrap();
        let book_manager = self.with_market_data.book_manager.unwrap();

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

        thread::spawn(move || loop {
            select! {
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
                }
            }
        });

        Ok(())
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

        config
            .solver
            .replace(Arc::new(ComponentLock::new(Solver::new(
                strategy,
                config.with_order_ids.clone(),
                basket_manager,
                price_tracker,
                order_book_manager,
                config.with_chain_connector.clone(),
                batch_manager,
                collateral_manager,
                config.with_index_order_manager.clone(),
                config.with_quote_request_manager.clone(),
                inventory_manager,
                config.max_batch_size,
                config.zero_threshold,
                config.client_order_wait_period,
                config.client_quote_wait_period,
            ))));

        Ok(config)
    }
}
