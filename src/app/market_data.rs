use std::sync::Arc;

use super::config::ConfigBuildError;
use binance_market_data::binance_subscriber::{
    BinanceOnlySubscriberTasks, BinanceSubscriberTaskConfig,
};
use derive_builder::Builder;
use eyre::{eyre, OptionExt, Result};
use market_data::market_data::RealMarketData;
use parking_lot::RwLock;
use rust_decimal::dec;
use symm_core::{
    core::{
        bits::Amount,
        functional::{
            IntoObservableManyArc, IntoObservableManyFun, IntoObservableSingle,
            IntoObservableSingleFun,
        },
        telemetry::{crossbeam::unbounded_traceable, TraceableEvent},
    },
    market_data::{
        market_data_connector::{MarketDataConnector, Subscription, MarketDataEvent},
        order_book::order_book_manager::PricePointBookManager,
        price_tracker::PriceTracker,
    },
};

#[derive(Clone, Builder)]
#[builder(
    pattern = "owned",
    build_fn(name = "try_build", error = "ConfigBuildError")
)]
pub struct MarketDataConfig {
    #[builder(setter(into, strip_option), default)]
    pub zero_threshold: Option<Amount>,

    #[builder(setter(into, strip_option), default)]
    pub max_subscriber_symbols: Option<usize>,

    #[builder(setter(into, strip_option), default)]
    pub subscription_check_period: Option<std::time::Duration>,

    #[builder(setter(into, strip_option), default)]
    pub subscription_limit_rate: Option<usize>,

    #[builder(setter(into, strip_option), default)]
    pub stale_check_period: Option<std::time::Duration>,

    #[builder(setter(into, strip_option), default)]
    pub stale_timeout: Option<chrono::Duration>,

    #[builder(setter(into), default)]
    pub subscriptions: Vec<Subscription>,

    #[builder(setter(into, strip_option), default)]
    pub with_price_tracker: Option<bool>,

    #[builder(setter(into, strip_option), default)]
    pub with_book_manager: Option<bool>,

    #[builder(setter(skip))]
    market_data: Option<Arc<RwLock<RealMarketData>>>,

    #[builder(setter(skip))]
    price_tracker: Option<Arc<RwLock<PriceTracker>>>,

    #[builder(setter(skip))]
    book_manager: Option<Arc<RwLock<PricePointBookManager>>>,
}

impl MarketDataConfig {
    #[must_use]
    pub fn builder() -> MarketDataConfigBuilder {
        MarketDataConfigBuilder::default()
    }

    pub fn expect_market_data_cloned(&self) -> Arc<RwLock<RealMarketData>> {
        self.market_data
            .clone()
            .ok_or(())
            .expect("Failed to get market data")
    }

    pub fn try_get_market_data_cloned(&self) -> Result<Arc<RwLock<RealMarketData>>> {
        self.market_data
            .clone()
            .ok_or_eyre("Failed to get market data")
    }

    pub fn expect_price_tracker_cloned(&self) -> Arc<RwLock<PriceTracker>> {
        self.price_tracker
            .clone()
            .ok_or(())
            .expect("Failed to get price tracker")
    }

    pub fn try_get_price_tracker_cloned(&self) -> Result<Arc<RwLock<PriceTracker>>> {
        self.price_tracker
            .clone()
            .ok_or_eyre("Failed to get price tracker")
    }

    pub fn expect_book_manager_cloned(&self) -> Arc<RwLock<PricePointBookManager>> {
        self.book_manager
            .clone()
            .ok_or(())
            .expect("Failed to get order book manager")
    }

    pub fn try_get_book_manager_cloned(&self) -> Result<Arc<RwLock<PricePointBookManager>>> {
        self.book_manager
            .clone()
            .ok_or_eyre("Failed to get order book manager")
    }

    pub fn start(&self) -> Result<()> {
        if let Some(market_data) = &self.market_data {
            market_data
                .write()
                .start()
                .map_err(|err| eyre!("Failed to start Market Data: {:?}", err))?;

            market_data
                .write()
                .subscribe(&self.subscriptions)
                .map_err(|err| eyre!("Failed to subscribe for market data: {:?}", err))?;

            Ok(())
        } else {
            Err(eyre!("Cannot start market data not configured"))
        }
    }

    pub fn start_with_event_handlers(&self) -> Result<()> {
        let market_data = self.try_get_market_data_cloned()?;
        let price_tracker = self.try_get_price_tracker_cloned()?;
        let book_manager = self.try_get_book_manager_cloned()?;

        let (market_data_tx, market_data_rx) =
            crossbeam::channel::unbounded::<Arc<MarketDataEvent>>();
        let (price_event_tx, price_event_rx) = crossbeam::channel::unbounded();
        let (book_event_tx, book_event_rx) = crossbeam::channel::unbounded();

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
                let _ = market_data_tx.send(event.clone());
            });

        std::thread::spawn(move || {
            loop {
                crossbeam::select! {
                    recv(market_data_rx) -> res => {
                        if let Ok(event) = res {
                            price_tracker.write().handle_market_data(&event);
                            book_manager.write().handle_market_data(&event);
                        }
                    },
                    recv(price_event_rx) -> res => {
                        if let Ok(_event) = res {
                            // optionally trace/log
                        }
                    },
                    recv(book_event_rx) -> res => {
                        if let Ok(_event) = res {
                            // optionally trace/log
                        }
                    }
                }
            }
        });

        self.start()?; // this starts the actual market data loop

        Ok(())
    }
}

impl MarketDataConfigBuilder {
    pub fn build(self) -> Result<MarketDataConfig, ConfigBuildError> {
        let mut config = self.try_build()?;

        let binance_subscriber_config = BinanceSubscriberTaskConfig {
            subscription_limit_rate: config.subscription_limit_rate.unwrap_or(3),

            stale_check_period: config
                .stale_check_period
                .unwrap_or(std::time::Duration::from_secs(10)),

            stale_timeout: config
                .stale_timeout
                .unwrap_or(chrono::Duration::seconds(60)),
        };

        let subscriber_task_factory =
            Arc::new(BinanceOnlySubscriberTasks::new(binance_subscriber_config));

        config
            .market_data
            .replace(Arc::new(RwLock::new(RealMarketData::new(
                config.max_subscriber_symbols.unwrap_or(10),
                config
                    .subscription_check_period
                    .unwrap_or(std::time::Duration::from_secs(20)),
                subscriber_task_factory,
            ))));

        if config.with_price_tracker.unwrap_or(true) {
            config
                .price_tracker
                .replace(Arc::new(RwLock::new(PriceTracker::new())));
        }

        if config.with_book_manager.unwrap_or(true) {
            config
                .book_manager
                .replace(Arc::new(RwLock::new(PricePointBookManager::new(
                    config.zero_threshold.unwrap_or(dec!(0.00001)),
                ))));
        }

        Ok(config)
    }
}
