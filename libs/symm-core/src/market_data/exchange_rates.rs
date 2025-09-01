use std::sync::Arc;

use eyre::eyre;
use itertools::Itertools;
use parking_lot::RwLock;

use crate::{
    core::bits::{Amount, PriceType, Symbol},
    market_data::price_tracker::PriceTracker,
};

pub trait ExchangeRates {
    fn get_exchange_rate(&self, from: Symbol, to: Symbol) -> eyre::Result<Amount>;
}

pub struct PriceTrackerExchangeRates {
    price_tracker: Arc<RwLock<PriceTracker>>,
}

impl PriceTrackerExchangeRates {
    pub fn new(price_tracker: Arc<RwLock<PriceTracker>>) -> Self {
        Self { price_tracker }
    }
}

impl ExchangeRates for PriceTrackerExchangeRates {
    fn get_exchange_rate(&self, from: Symbol, to: Symbol) -> eyre::Result<Amount> {
        let pair = format!("{}{}", from, to);
        let res = self
            .price_tracker
            .read()
            .get_prices(PriceType::VolumeWeighted, &[Symbol::from(pair)]);

        if !res.missing_symbols.is_empty() {
            Err(eyre!(
                "Missing price for: {}",
                res.missing_symbols.iter().map(|x| x.to_string()).join(",")
            ))?;
        }

        let (_, &price) = res.prices.iter().exactly_one().unwrap();
        Ok(price)
    }
}
