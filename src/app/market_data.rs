use std::sync::Arc;

use super::config::ConfigBuildError;
use binance_market_data::binance_market_data::BinanceMarketData;
use derive_builder::Builder;
use eyre::{eyre, Result};
use parking_lot::RwLock;
use rust_decimal::dec;
use symm_core::{
    core::bits::{Amount, Symbol},
    market_data::{
        market_data_connector::MarketDataConnector,
        order_book::order_book_manager::PricePointBookManager, price_tracker::PriceTracker,
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

    #[builder(setter(into), default)]
    pub symbols: Vec<Symbol>,

    #[builder(setter(into, strip_option), default)]
    pub with_price_tracker: Option<bool>,

    #[builder(setter(into, strip_option), default)]
    pub with_book_manager: Option<bool>,

    #[builder(setter(skip))]
    pub(crate) market_data: Option<Arc<RwLock<BinanceMarketData>>>,

    #[builder(setter(skip))]
    pub(crate) price_tracker: Option<Arc<RwLock<PriceTracker>>>,

    #[builder(setter(skip))]
    pub(crate) book_manager: Option<Arc<RwLock<PricePointBookManager>>>,
}

impl MarketDataConfig {
    #[must_use]
    pub fn builder() -> MarketDataConfigBuilder {
        MarketDataConfigBuilder::default()
    }

    pub fn start(&self) -> Result<()> {
        if let Some(market_data) = &self.market_data {
            market_data
                .write()
                .start()
                .map_err(|err| eyre!("Failed to start Market Data: {:?}", err))?;

            market_data
                .write()
                .subscribe(&self.symbols)
                .map_err(|err| eyre!("Failed to subscribe for market data: {:?}", err))?;

            Ok(())
        } else {
            Err(eyre!("Cannot start market data not configured"))
        }
    }
}

impl MarketDataConfigBuilder {
    pub fn build(self) -> Result<MarketDataConfig, ConfigBuildError> {
        let mut config = self.try_build()?;

        config
            .market_data
            .replace(Arc::new(RwLock::new(BinanceMarketData::new(
                config.max_subscriber_symbols.unwrap_or(100),
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
