use std::{collections::HashMap, sync::Arc};

use eyre::{eyre, OptionExt};
use itertools::Itertools;
use parking_lot::RwLock;

use crate::{
    core::bits::{Amount, PriceType, Symbol},
    market_data::price_tracker::PriceTracker,
};

pub trait ExchangeRates {
    fn get_exchange_rate(&self, from: Symbol, to: Symbol) -> eyre::Result<Amount>;
}

pub struct FixedExchangeRates {
    rates: HashMap<Symbol, Amount>,
}

impl FixedExchangeRates {
    pub fn new(rates: HashMap<Symbol, Amount>) -> Self {
        Self { rates }
    }
}

impl ExchangeRates for FixedExchangeRates {
    fn get_exchange_rate(&self, from: Symbol, to: Symbol) -> eyre::Result<Amount> {
        if from == to {
            return Ok(Amount::ONE);
        }

        let pair = Symbol::from(format!("{}{}", from, to));

        let rate = self.rates.get(&pair).ok_or_eyre("Missing price")?;
        Ok(*rate)
    }
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
        if from == to {
            return Ok(Amount::ONE);
        }

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
