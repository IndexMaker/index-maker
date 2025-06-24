use std::sync::Arc;

use super::config::ConfigBuildError;
use binance_market_data::binance_market_data::BinanceMarketData;
use derive_builder::Builder;
use eyre::{eyre, Result};
use parking_lot::RwLock;
use rust_decimal::dec;
use symm_core::{
    core::{
        bits::{Amount, Symbol},
        functional::IntoObservableManyArc,
    },
    market_data::{
        market_data_connector::{MarketDataConnector, MarketDataEvent},
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

    #[builder(setter(into), default)]
    pub symbols: Vec<Symbol>,

}

impl MarketDataConfig {
    #[must_use]
    pub fn builder() -> MarketDataConfigBuilder {
        MarketDataConfigBuilder::default()
    }

    pub fn make(
        self,
    ) -> Result<(
        Arc<RwLock<BinanceMarketData>>,
        Arc<RwLock<PriceTracker>>,
        Arc<RwLock<PricePointBookManager>>,
    )> {
        let price_tracker = Arc::new(RwLock::new(PriceTracker::new()));
        let book_manager = Arc::new(RwLock::new(PricePointBookManager::new(
            self.zero_threshold.unwrap_or(dec!(0.00001)),
        )));

        let market_data = Arc::new(RwLock::new(BinanceMarketData::new(100)));

        let price_tracker_clone = price_tracker.clone();
        let book_manager_clone = book_manager.clone();

        market_data
            .write()
            .get_multi_observer_arc()
            .write()
            .add_observer_fn(move |event: &Arc<MarketDataEvent>| {
                price_tracker_clone.write().handle_market_data(&*event);
                book_manager_clone.write().handle_market_data(&*event);
            });

        market_data
            .write()
            .start()
            .map_err(|err| eyre!("Failed to start Market Data: {:?}", err))?;

        market_data
            .write()
            .subscribe(&self.symbols)
            .map_err(|err| eyre!("Failed to subscribe for market data: {:?}", err))?;

        Ok((market_data, price_tracker, book_manager))
    }
}
